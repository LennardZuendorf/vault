use super::{AppState, CHAT_TASK_ID};
use crate::error::{Error, Result};
use base64::Engine;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use tauri::AppHandle;

#[tauri::command]
pub async fn save_chat_image(app: AppHandle, pane_id: String, name: String, data: String) -> Result<String> {
    super::blocking(app, move |state| save_chat_image_inner(state, &pane_id, &name, &data)).await
}

fn invalid(message: &str) -> Error {
    Error::Other(message.into())
}

fn save_chat_image_inner(state: &AppState, pane_id: &str, name: &str, data: &str) -> Result<String> {
    let path = Path::new(name);
    if name.chars().any(char::is_control)
        || !matches!((path.components().next(), path.components().nth(1)), (Some(Component::Normal(_)), None))
        || name.contains('\\')
    {
        return Err(invalid("Invalid image filename."));
    }
    let limit = crate::integrations::github::MAX_BYTES;
    if data.len() > limit.div_ceil(3) * 4 {
        return Err(invalid("Image is larger than 10 MiB."));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| invalid("Invalid image data."))?;
    if bytes.len() > limit {
        return Err(invalid("Image is larger than 10 MiB."));
    }
    let ext =
        crate::integrations::github::kind_of(&bytes).ok_or_else(|| invalid("The file is not a supported image."))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("Invalid image filename."))?;

    let pane = state.ptys.info(pane_id)?;
    // A terminal takes them too (PANE-19): its agent is handed the path as
    // if it had been pasted, which is how Claude Code is given an image.
    if !pane.running {
        return Err(invalid("The pane is not running."));
    }
    let root = if pane.task_id == CHAT_TASK_ID {
        let chats = state.config.worktree_root().join("_chat").canonicalize()?;
        let room = Path::new(&pane.cwd).canonicalize()?;
        if room.parent() != Some(chats.as_path()) {
            return Err(invalid("Invalid chat room."));
        }
        room
    } else {
        PathBuf::from(state.config.task(&pane.task_id)?.root).canonicalize()?
    };
    let attachments = root.join("attachments");
    for checkout in state.config.read().checkouts {
        let candidate = PathBuf::from(checkout.path);
        let resolved = match candidate.canonicalize() {
            Ok(path) => path,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = candidate.parent().ok_or_else(|| invalid("Invalid checkout path."))?;
                let leaf = candidate.file_name().ok_or_else(|| invalid("Invalid checkout path."))?;
                match parent.canonicalize() {
                    Ok(parent) => parent.join(leaf),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => candidate,
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        };
        if root.starts_with(&resolved) || attachments.starts_with(&resolved) {
            return Err(invalid("Attachments cannot be saved inside a checkout."));
        }
    }
    let root_dir = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(&root)?;
    let folder = CString::new("attachments").map_err(|_| invalid("Invalid attachment folder."))?;
    // mkdirat and openat keep the destination anchored to the verified root.
    let made = unsafe { libc::mkdirat(root_dir.as_raw_fd(), folder.as_ptr(), 0o700) };
    if made != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    let fd = unsafe {
        libc::openat(
            root_dir.as_raw_fd(),
            folder.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let dir = unsafe { File::from_raw_fd(fd) };
    let opened = dir.metadata()?;
    let visible = std::fs::symlink_metadata(&attachments)?;
    if !visible.is_dir() || opened.dev() != visible.dev() || opened.ino() != visible.ino() {
        return Err(invalid("Attachment folder changed."));
    }
    store_image(&dir, &attachments, stem, ext, |file| file.write_all(&bytes))
}

fn store_image(
    dir: &File,
    attachments: &Path,
    stem: &str,
    ext: &str,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<String> {
    let name = CString::new(format!(".layer-image-{}", uuid::Uuid::new_v4()))
        .map_err(|_| invalid("Invalid staging folder."))?;
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
        return Err(error.into());
    }
    let stage = Staging { parent: dir, name, dir: unsafe { File::from_raw_fd(fd) } };
    let image = c"image";
    let fd = unsafe {
        libc::openat(
            stage.dir.as_raw_fd(),
            image.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    write(&mut file)?;
    for n in 1..=10_000 {
        let filename = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} {n}.{ext}") };
        let c_name = CString::new(filename.as_bytes()).map_err(|_| invalid("Invalid image filename."))?;
        let linked =
            unsafe { libc::linkat(stage.dir.as_raw_fd(), image.as_ptr(), dir.as_raw_fd(), c_name.as_ptr(), 0) };
        if linked != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error.into());
        }
        return Ok(attachments.join(filename).to_string_lossy().into_owned());
    }
    Err(invalid("Too many images with that name."))
}

struct Staging<'a> {
    parent: &'a File,
    name: CString,
    dir: File,
}

impl Drop for Staging<'_> {
    fn drop(&mut self) {
        unsafe {
            libc::unlinkat(self.dir.as_raw_fd(), c"image".as_ptr(), 0);
            libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), libc::AT_REMOVEDIR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acp,
        pty::{PaneKind, SpawnOptions},
    };
    use base64::Engine;
    use std::path::Path;

    #[test]
    fn a_failed_write_never_exposes_a_destination_or_removes_an_existing_file() {
        let root = std::env::temp_dir().join(format!("vl-attachments-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("x.png"), b"existing").unwrap();
        let dir = File::open(&root).unwrap();
        let exposed = std::cell::Cell::new(false);
        let result = store_image(&dir, &root, "x", "png", |file| {
            file.write_all(b"partial")?;
            exposed.set(root.join("x 2.png").exists());
            Err(std::io::Error::other("injected write failure"))
        });
        assert!(result.is_err());
        assert!(!exposed.get(), "a partial destination was visible during writing");
        assert_eq!(std::fs::read(root.join("x.png")).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn pane(state: &AppState, task: &str, cwd: &Path, checkout: Option<&str>) -> String {
        let app = tauri::test::mock_app();
        let opts = SpawnOptions {
            task_id: task.into(),
            checkout_id: checkout.map(str::to_owned),
            cwd: cwd.to_string_lossy().into_owned(),
            kind: PaneKind::Agent,
            title: "Fake".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), crate::pty::acp::tests::FAKE_AGENT.into()],
            agent_id: Some("fake".into()),
            rows: None,
            cols: None,
            initial_input: None,
            prompted: true,
            env: vec![],
            title_activity: None,
            title_topic: None,
        };
        let launch = acp::Launch {
            mcp: None,
            pick: acp::Pick::New,
            prompt: Some("ping".into()),
            presets: vec![],
            meta: None,
            on_session: None,
        };
        state.ptys.spawn_acp(app.handle(), opts, launch).unwrap().id
    }

    #[test]
    fn saves_repo_pane_images_at_task_root_without_overwriting() {
        let (root, mut cfg) = super::super::repos::tests::setup();
        super::super::repos::tests::add_task(&mut cfg, &root, "task");
        let task = root.join("tasks/task");
        std::fs::create_dir_all(task.join("api")).unwrap();
        let state = super::super::repos::tests::state(&root, cfg);
        let id = pane(&state, "task", &task.join("api"), Some("task-api"));
        let bytes = b"\x89PNG\r\n\x1a\nimage";
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        let first = save_chat_image_inner(&state, &id, "Screen Shot.png", &data).unwrap();
        let second = save_chat_image_inner(&state, &id, "Screen Shot.png", &data).unwrap();
        assert_eq!(std::path::Path::new(&first), task.join("attachments/Screen Shot.png"));
        assert_eq!(std::path::Path::new(&second), task.join("attachments/Screen Shot 2.png"));
        assert_eq!(std::fs::read(first).unwrap(), bytes);
        assert_eq!(std::fs::read(second).unwrap(), bytes);
        state.ptys.close(&id).unwrap();
    }

    #[test]
    fn a_terminal_pane_saves_dropped_images_at_its_task_root() {
        let (root, mut cfg) = super::super::repos::tests::setup();
        super::super::repos::tests::add_task(&mut cfg, &root, "task");
        let task = root.join("tasks/task");
        std::fs::create_dir_all(task.join("api")).unwrap();
        let state = super::super::repos::tests::state(&root, cfg);
        let app = tauri::test::mock_app();
        let opts = SpawnOptions {
            task_id: "task".into(),
            checkout_id: Some("task-api".into()),
            cwd: task.join("api").to_string_lossy().into_owned(),
            kind: PaneKind::Agent,
            title: "Claude Code".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            agent_id: Some("claude".into()),
            rows: None,
            cols: None,
            initial_input: None,
            prompted: true,
            env: vec![],
            title_activity: None,
            title_topic: None,
        };
        let id = state.ptys.spawn(app.handle(), opts).unwrap().id;
        let bytes = b"\x89PNG\r\n\x1a\nimage";
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        let saved = save_chat_image_inner(&state, &id, "Screen Shot.png", &data).unwrap();
        assert_eq!(std::path::Path::new(&saved), task.join("attachments/Screen Shot.png"));
        assert_eq!(std::fs::read(saved).unwrap(), bytes);
        state.ptys.close(&id).unwrap();
    }

    #[test]
    fn standalone_rooms_keep_separate_attachments() {
        let (root, cfg) = super::super::repos::tests::setup();
        let rooms = [root.join("tasks/_chat/one"), root.join("tasks/_chat/two")];
        for room in &rooms {
            std::fs::create_dir_all(room).unwrap();
        }
        let state = super::super::repos::tests::state(&root, cfg);
        let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nx");
        for room in &rooms {
            let id = pane(&state, "chat", room, None);
            let saved = save_chat_image_inner(&state, &id, "x.png", &data).unwrap();
            assert_eq!(Path::new(&saved), room.join("attachments/x.png"));
            state.ptys.close(&id).unwrap();
        }
    }

    #[test]
    fn rejects_bad_inputs_and_missing_or_stopped_panes_without_creating_files() {
        let (root, mut cfg) = super::super::repos::tests::setup();
        super::super::repos::tests::add_task(&mut cfg, &root, "task");
        let task = root.join("tasks/task");
        std::fs::create_dir_all(task.join("api")).unwrap();
        let state = super::super::repos::tests::state(&root, cfg);
        let id = pane(&state, "task", &task.join("api"), Some("task-api"));
        let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nx");
        for name in ["../shot.png", "bad\n.png"] {
            assert!(save_chat_image_inner(&state, &id, name, &data).is_err());
        }
        assert!(save_chat_image_inner(&state, &id, "fake.png", "bm90IGEgcG5n").is_err());
        assert!(save_chat_image_inner(&state, &id, "bad.png", "%%%").is_err());
        assert!(save_chat_image_inner(&state, &id, "large.png", &"A".repeat(14_000_000)).is_err());
        assert!(save_chat_image_inner(&state, "missing", "x.png", &data).is_err());
        state.ptys.kill(&id).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.ptys.info(&id).unwrap().running && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!state.ptys.info(&id).unwrap().running);
        assert!(save_chat_image_inner(&state, &id, "x.png", &data).is_err());
        state.ptys.close(&id).unwrap();
        assert!(save_chat_image_inner(&state, &id, "x.png", &data).is_err());
        assert!(!task.join("attachments").exists());
    }

    #[test]
    fn the_saved_extension_comes_from_the_image_signature() {
        let (root, mut cfg) = super::super::repos::tests::setup();
        super::super::repos::tests::add_task(&mut cfg, &root, "task");
        let task = root.join("tasks/task");
        std::fs::create_dir_all(task.join("api")).unwrap();
        let state = super::super::repos::tests::state(&root, cfg);
        let id = pane(&state, "task", &task.join("api"), Some("task-api"));
        let data = base64::engine::general_purpose::STANDARD.encode(b"GIF89ax");
        let path = save_chat_image_inner(&state, &id, "photo.png", &data).unwrap();
        assert_eq!(Path::new(&path), task.join("attachments/photo.gif"));
        state.ptys.close(&id).unwrap();
    }

    #[test]
    fn refuses_checkout_collisions_and_symlinked_directories() {
        use std::os::unix::fs::symlink;
        for collision in ["root", "attachments", "symlink"] {
            let (root, mut cfg) = super::super::repos::tests::setup();
            super::super::repos::tests::add_task(&mut cfg, &root, "task");
            let task = root.join("tasks/task");
            std::fs::create_dir_all(task.join("api")).unwrap();
            if collision == "root" {
                cfg.checkouts[0].path = task.to_string_lossy().into_owned();
            }
            if collision == "attachments" {
                std::fs::create_dir(task.join("attachments")).unwrap();
                cfg.checkouts[0].path = task.join("attachments").to_string_lossy().into_owned();
            }
            let outside = root.join("outside");
            std::fs::create_dir(&outside).unwrap();
            if collision == "symlink" {
                symlink(&outside, task.join("attachments")).unwrap();
            }
            let state = super::super::repos::tests::state(&root, cfg);
            let id = pane(&state, "task", &task.join("api"), Some("task-api"));
            let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nx");
            assert!(save_chat_image_inner(&state, &id, "x.png", &data).is_err(), "{collision}");
            assert!(!outside.join("x.png").exists());
            assert!(!task.join("attachments/x.png").exists());
            state.ptys.close(&id).unwrap();
        }
    }

    #[test]
    fn a_filename_symlink_is_never_followed_or_overwritten() {
        use std::os::unix::fs::symlink;
        let (root, mut cfg) = super::super::repos::tests::setup();
        super::super::repos::tests::add_task(&mut cfg, &root, "task");
        let task = root.join("tasks/task");
        std::fs::create_dir_all(task.join("api")).unwrap();
        std::fs::create_dir(task.join("attachments")).unwrap();
        let outside = root.join("outside.png");
        std::fs::write(&outside, b"unchanged").unwrap();
        symlink(&outside, task.join("attachments/x.png")).unwrap();
        let state = super::super::repos::tests::state(&root, cfg);
        let id = pane(&state, "task", &task.join("api"), Some("task-api"));
        let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nx");
        let result = save_chat_image_inner(&state, &id, "x.png", &data);
        assert!(result.is_err() || result.unwrap().ends_with("/x 2.png"));
        assert_eq!(std::fs::read(outside).unwrap(), b"unchanged");
        assert!(std::fs::symlink_metadata(task.join("attachments/x.png")).unwrap().file_type().is_symlink());
        state.ptys.close(&id).unwrap();
    }
}
