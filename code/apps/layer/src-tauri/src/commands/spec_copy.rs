//! A task's spec as plain files at the task root (§19): `<task>/spec/`, laid
//! out the way the task's toolset lays it out. This is the one place that
//! reads and writes them, and it moves a spec from where earlier builds kept
//! it, and keeps it when its task is deleted.
//!
//! The folder is where agents and the user edit, so nothing here may be
//! talked into touching anything outside it: a path with `..`, a file the
//! toolset does not list, and any symlink between the folder and the file
//! are refused. Linux and macOS agents do make links (an install, a skill),
//! and a write through one would land wherever it points.

// Later tasks call most of this; until then only the tests and the delete do.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::Mutex;

use crate::config::Task;
use crate::error::{Error, Result};
use crate::spec::toolset::{self, Toolset};
use crate::spec::{self, glob, Kind, Part};

use super::panes::agent_file_dir;
use super::spec::{spec_of, APPROVED, LEGACY_FILE};
use super::AppState;

/// The spec's folder in the task folder.
const WORKING: &str = "spec";

/// What a task uses when neither it nor the app says.
const DEFAULT_TOOLSET: &str = "kiro";

/// Walks stop here, so a spec folder that grew a `node_modules` of its own
/// (an install's output) cannot make a listing take minutes.
pub(crate) const MOST_FILES: usize = 5000;
pub(crate) const MOST_DEPTH: usize = 16;

/// Every write and every move takes this one lock. Two agents saving at once
/// then write one after the other, and a move checks that the folder is
/// absent and makes it without another one making it in between.
static WRITING: Mutex<()> = Mutex::new(());

/// The toolset of `task`: its own, else the app's default, else Kiro. An id
/// nothing has (a custom toolset taken out of the settings) is passed over
/// rather than failing every read of the spec.
pub(crate) fn toolset_of(state: &AppState, task: &Task) -> Toolset {
    state
        .config
        .with(|c| {
            [task.toolset.as_deref(), c.default_toolset.as_deref(), Some(DEFAULT_TOOLSET)]
                .into_iter()
                .flatten()
                .find_map(|id| toolset::find(id, &c.toolsets))
        })
        .unwrap_or_else(bare_kiro)
}

/// Kiro's layout without its manifest, for the day the built-in one cannot be
/// read (a test fails first). Nothing here may panic.
fn bare_kiro() -> Toolset {
    Toolset {
        id: DEFAULT_TOOLSET.into(),
        name: "Kiro (Layer)".into(),
        version: None,
        files: ["requirements.md", "bugfix.md", "design.md", "tasks.md"].map(String::from).to_vec(),
        steps: vec!["tasks.md".into()],
        install: Vec::new(),
        commit_to: Some("specs/{task}".into()),
        agents_md: String::new(),
        extras: Vec::new(),
    }
}

/// Where `task`'s spec is: `spec/` in its folder, or, for a task whose root
/// is itself a checkout (from before tasks had folders), the config folder's
/// `specs/<repository>/<task>/`, where specs kept by the app always were.
pub(crate) fn spec_dir(state: &AppState, task: &Task) -> PathBuf {
    working_dir(state, task).unwrap_or_else(|| kept_dir(state, task))
}

/// `<task>/spec/`, when the task has a folder of its own to put it in and no
/// repository is checked out there under that name.
fn working_dir(state: &AppState, task: &Task) -> Option<PathBuf> {
    let dir = agent_file_dir(state, task)?.join(WORKING);
    let taken = state.config.checkouts_of(&task.id).iter().any(|c| Path::new(&c.path) == dir);
    (!taken).then_some(dir)
}

fn kept_dir(state: &AppState, task: &Task) -> PathBuf {
    let repository = state
        .config
        .checkouts_of(&task.id)
        .into_iter()
        .find_map(|c| state.config.project(&c.project_id).ok())
        .map(|p| spec::folder_name(&p.name))
        .unwrap_or_else(|| "task".into());
    state.config.folder().join("specs").join(repository).join(spec::folder_name(&task.branch))
}

fn refused(what: impl std::fmt::Display) -> Error {
    Error::Other(what.to_string())
}

/// `rel` as a file `t` keeps in a spec, under `dir`.
fn target(dir: &Path, t: &Toolset, rel: &str) -> Result<PathBuf> {
    if !glob::is_safe_rel(rel) {
        return Err(refused(format!("{rel:?} is not a path inside the spec folder")));
    }
    if !t.files.iter().any(|g| glob::matches(g, rel)) {
        return Err(refused(format!("{rel} is not a file {} keeps in a spec", t.name)));
    }
    Ok(dir.join(rel))
}

/// Refuse when `dir`, or any folder or file between it and `rel`, is a
/// symlink. What does not exist yet is fine: it is made as a plain folder.
/// `lstat` is the point: a link is looked at, never followed.
fn refuse_links(dir: &Path, rel: &str) -> Result<()> {
    let mut at = dir.to_path_buf();
    let below = if rel.is_empty() { Vec::new() } else { rel.split('/').collect() };
    for segment in std::iter::once(None).chain(below.into_iter().map(Some)) {
        if let Some(segment) = segment {
            at.push(segment);
        }
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(refused(format!("{} is a link, and a spec never goes through one", at.display())));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// The files of the spec `t` keeps, as paths relative to `dir`, sorted. Links
/// are not followed and not listed. A folder that is not there has none.
pub(crate) fn list_files(dir: &Path, t: &Toolset) -> Result<Vec<String>> {
    refuse_links(dir, "")?;
    let mut found = Vec::new();
    walk(dir, "", 0, t, &mut found)?;
    // Plain byte order, whatever the Mac's locale says (the listing is
    // compared and committed).
    found.sort();
    Ok(found)
}

fn walk(dir: &Path, prefix: &str, depth: usize, t: &Toolset, found: &mut Vec<String>) -> Result<()> {
    let entries = match std::fs::read_dir(dir.join(prefix)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if depth < MOST_DEPTH && name != ".git" && name != "node_modules" {
                walk(dir, &rel, depth + 1, t, found)?;
            }
        } else if kind.is_file() && t.files.iter().any(|g| glob::matches(g, &rel)) && found.len() < MOST_FILES {
            found.push(rel);
        }
    }
    Ok(())
}

/// One file of the spec.
pub(crate) fn read_file(dir: &Path, t: &Toolset, rel: &str) -> Result<String> {
    let path = target(dir, t, rel)?;
    refuse_links(dir, rel)?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => Ok(std::fs::read_to_string(&path)?),
        Ok(_) => Err(refused(format!("{rel} is not a file"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(rel.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// Replace the whole text of one file of the spec, making its folders. The
/// text is written to a new file beside it, which can only be one nobody
/// else has made (`create_new`), and renamed over it: a reader sees the old
/// text or the new, never half.
pub(crate) fn write_file(dir: &Path, t: &Toolset, rel: &str, text: &str) -> Result<()> {
    target(dir, t, rel)?;
    write_under(dir, rel, text)
}

/// `write_file` for a path already known to be one the caller may write:
/// Commit spec's, which are a repository's, not the spec folder's.
pub(crate) fn write_under(dir: &Path, rel: &str, text: &str) -> Result<()> {
    if !glob::is_safe_rel(rel) {
        return Err(refused(format!("{rel:?} is not a path inside {}", dir.display())));
    }
    let path = dir.join(rel);
    let _writing = WRITING.lock();
    refuse_links(dir, rel)?;
    let parent = path.parent().ok_or_else(|| refused(format!("{rel} has no folder")))?;
    std::fs::create_dir_all(parent)?;
    // Checked again: what was made is real folders, and nothing swapped one
    // for a link while they were.
    refuse_links(dir, rel)?;
    let temp = parent.join(format!(".layer-{}.tmp", uuid::Uuid::new_v4()));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(written?)
}

// ---- Moving a spec from where it was kept before -------------------------

/// A file's text, when it has any besides white space.
fn read_text(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// The approved text of a file, or the draft written since when that is
/// newer (a draft is dropped when it is approved, so one that is there was
/// written after, unless the approval was later still).
fn newer(approved: &Path, draft: &Path) -> Option<String> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    match (read_text(approved), read_text(draft)) {
        (Some(a), Some(d)) => Some(if modified(draft) > modified(approved) { d } else { a }),
        (a, d) => a.or(d),
    }
}

/// Each repository's tasks as one `tasks.md`: a `## <folder>` section per
/// repository, which is how a step file says whose steps are whose.
fn merged(sections: Vec<(String, String)>) -> Option<String> {
    (!sections.is_empty()).then(|| {
        sections
            .iter()
            .map(|(folder, text)| format!("## {folder}\n\n{}\n", text.trim_end()))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// What `task`'s spec was, as files of Kiro's layout, from where earlier
/// builds kept it: what was approved with the drafts written since; else the
/// repositories' own copies; else the single `SPEC.md` (SPEC-17).
fn old_files(state: &AppState, task: &Task, dest: &Path) -> Vec<(String, String)> {
    let spec = spec_of(state, task);
    let kind = spec.kind();
    let (requirements, design, tasks) =
        (Part::Requirements.file(kind), Part::Design.file(kind), Part::Tasks.file(kind));
    let mut out: Vec<(String, String)> = Vec::new();
    let keep = |out: &mut Vec<_>, file: &str, text: Option<String>| {
        if let Some(text) = text {
            out.push((file.to_string(), text));
        }
    };

    if let Some(drafts) = &spec.drafts {
        let approved = drafts.join(APPROVED);
        // A draft is saved under the plain name, whether the spec is a bug's or not.
        let draft_requirements = drafts.join(Part::Requirements.file(Kind::Feature));
        keep(&mut out, requirements, newer(&approved.join(requirements), &draft_requirements));
        keep(&mut out, design, newer(&approved.join(design), &drafts.join(design)));
        let sections = spec
            .homes
            .iter()
            .filter_map(|h| {
                let text = newer(&approved.join(&h.folder).join(tasks), &drafts.join(&h.folder).join(tasks))?;
                Some((h.folder.clone(), text))
            })
            .collect();
        keep(&mut out, tasks, merged(sections));
    }
    if !out.is_empty() {
        return out;
    }

    // The first repository's requirements and design, and each one's tasks.
    let copies: Vec<_> = spec.homes.iter().filter(|h| h.dir != dest).collect();
    let first = |file: &str| copies.iter().find_map(|h| read_text(&h.dir.join(file)));
    keep(&mut out, requirements, first(requirements));
    keep(&mut out, design, first(design));
    let sections = copies.iter().filter_map(|h| Some((h.folder.clone(), read_text(&h.dir.join(tasks))?))).collect();
    keep(&mut out, tasks, merged(sections));
    if !out.is_empty() {
        return out;
    }

    if let Some(text) = agent_file_dir(state, task).and_then(|d| read_text(&d.join(LEGACY_FILE))) {
        out.push((Part::Requirements.file(Kind::Feature).to_string(), text));
    }
    out
}

/// Put `task`'s spec from where earlier builds kept it into its folder, when
/// the folder is not there yet. True when it did. Idempotent: the folder is
/// built whole beside it and renamed into place, so a move that stopped
/// halfway leaves no folder and the next call does it again, and a folder
/// that is there is never touched. What it moves is Kiro's layout, so the
/// task is Kiro's from then on, whatever the app's default has become.
pub(crate) fn move_old_spec(state: &AppState, task: &Task) -> Result<bool> {
    let _writing = WRITING.lock();
    let dest = spec_dir(state, task);
    if std::fs::symlink_metadata(&dest).is_ok() {
        return Ok(false);
    }
    let files = old_files(state, task, &dest);
    if files.is_empty() {
        return Ok(false);
    }
    if task.toolset.is_none() {
        state.config.update(|c| {
            if let Some(t) = c.tasks.iter_mut().find(|t| t.id == task.id && t.toolset.is_none()) {
                t.toolset = Some(DEFAULT_TOOLSET.into());
            }
        })?;
    }
    let parent = dest.parent().ok_or_else(|| refused("the spec has no folder to be in"))?;
    std::fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".spec-moving-{}", uuid::Uuid::new_v4()));
    let moved = (|| -> std::io::Result<()> {
        std::fs::create_dir(&stage)?;
        for (file, text) in &files {
            std::fs::write(stage.join(file), text)?;
        }
        std::fs::rename(&stage, &dest)
    })();
    if moved.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    moved?;
    Ok(true)
}

// ---- Keeping a spec when its task goes ------------------------------------

/// Copy `task`'s spec folder to the config folder's `specs/<task>/`, so a
/// spec never committed (or kept off the repository) outlives the task on
/// this Mac (SPEC-3). It copies and leaves the folder: removing it is the
/// caller's, once this has succeeded. A spec already kept there (a task with
/// no folder of its own) or without a file is not copied. A name taken by
/// another task's archive gets `-2`, `-3`, and so on.
pub(crate) fn archive_spec(state: &AppState, task: &Task) -> Result<()> {
    let Some(src) = working_dir(state, task) else { return Ok(()) };
    archive_folder(state.config.folder(), &src, &spec::folder_name(&task.branch))
}

/// The same for a spec in a task folder no task uses any more (Clean up):
/// `dir/spec`, kept under the folder's own name.
pub(crate) fn archive_leftover(config_folder: &Path, dir: &Path) -> Result<()> {
    let name = dir.file_name().map(|n| spec::folder_name(&n.to_string_lossy())).unwrap_or_else(|| "task".into());
    archive_folder(config_folder, &dir.join(WORKING), &name)
}

/// Copy the files of the spec folder `src` into `<config folder>/specs/<name>/`.
/// It fails, and keeps nothing, when it cannot take all of it: the caller
/// removes the folder next, and what was not copied would be gone.
fn archive_folder(config_folder: &Path, src: &Path, name: &str) -> Result<()> {
    if !std::fs::symlink_metadata(src).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    let _writing = WRITING.lock();
    let kept = config_folder.join("specs");
    // `kept` is made by the first file copied into it, so a spec with no file
    // leaves nothing behind.
    let stage = kept.join(format!(".archiving-{}", uuid::Uuid::new_v4()));
    let result = match copy_files(src, &stage, 0, &mut 0) {
        Ok(0) => Ok(()),
        Ok(_) => (1..100)
            .map(|n| if n == 1 { kept.join(name) } else { kept.join(format!("{name}-{n}")) })
            .find(|p| std::fs::symlink_metadata(p).is_err())
            .ok_or_else(|| refused(format!("there are 99 saved specs called {name} already")))
            .and_then(|to| Ok(std::fs::rename(&stage, to)?)),
        Err(e) => Err(e),
    };
    // A stage that was not renamed away is a partial copy.
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Take `task`'s spec folder out of its task folder, once `archive_spec` kept
/// it. Never the config folder's copy, and a link is removed, not followed.
pub(crate) fn remove_spec(state: &AppState, task: &Task) -> Result<()> {
    let Some(dir) = working_dir(state, task) else { return Ok(()) };
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Ok(std::fs::remove_dir_all(&dir)?),
        Ok(_) => Ok(std::fs::remove_file(&dir)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Copy the regular files under `from` into `to`, making the folders they
/// need and no others; how many were copied. Links are not followed and not
/// copied, and a `node_modules` is not kept: it is what an install writes and
/// can be written again. A folder nested deeper than `MOST_DEPTH`, or more
/// than `MOST_FILES` files, is an error, not a silent gap in the copy.
fn copy_files(from: &Path, to: &Path, depth: usize, total: &mut usize) -> Result<usize> {
    let mut copied = 0;
    for entry in std::fs::read_dir(from)?.flatten() {
        let kind = entry.file_type()?;
        let into = to.join(entry.file_name());
        if kind.is_dir() {
            if entry.file_name() == "node_modules" {
                continue;
            }
            if depth >= MOST_DEPTH {
                return Err(refused(format!("{} is nested too deeply to keep", entry.path().display())));
            }
            copied += copy_files(&entry.path(), &into, depth + 1, total)?;
        } else if kind.is_file() {
            if *total >= MOST_FILES {
                return Err(refused(format!("{} holds more than {MOST_FILES} files to keep", from.display())));
            }
            std::fs::create_dir_all(to)?;
            std::fs::copy(entry.path(), &into)?;
            *total += 1;
            copied += 1;
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec::{APPROVED, DRAFTS};
    use crate::commands::spec_world::{get, put, world};

    fn named(id: &str) -> Toolset {
        toolset::find(id, &[]).unwrap()
    }

    #[test]
    fn a_task_without_a_toolset_uses_the_app_default_then_kiro() {
        let (root, state, mut task) = world(&["api"]);
        assert_eq!(toolset_of(&state, &task).id, "kiro");

        state.config.update(|c| c.default_toolset = Some("openspec".into())).unwrap();
        assert_eq!(toolset_of(&state, &task).id, "openspec");

        task.toolset = Some("superpowers".into());
        assert_eq!(toolset_of(&state, &task).id, "superpowers");

        // An id nothing here has is passed over, not an error.
        task.toolset = Some("gone".into());
        assert_eq!(toolset_of(&state, &task).id, "openspec");
        state.config.update(|c| c.default_toolset = Some("also-gone".into())).unwrap();
        assert_eq!(toolset_of(&state, &task).id, "kiro");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_spec_is_a_folder_in_the_task_or_in_the_config_folder_when_the_task_has_none() {
        let (root, state, task) = world(&["api"]);
        assert_eq!(spec_dir(&state, &task), root.join("tasks/T-1/spec"));

        // An older task whose root is its one checkout.
        let mut old = task.clone();
        old.root = root.join("tasks/T-1/api").to_string_lossy().to_string();
        assert_eq!(spec_dir(&state, &old), root.join("specs/api/T-1"));

        // A repository that is itself called `spec` is where the folder would be.
        let (root2, state2, task2) = world(&["spec"]);
        assert_eq!(spec_dir(&state2, &task2), root2.join("specs/spec/T-1"));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&root2).ok();
    }

    #[test]
    fn writing_outside_the_spec_folder_is_refused() {
        let (root, _, _) = world(&["api"]);
        let dir = root.join("spec");
        let kiro = named("kiro");
        for rel in ["../x.md", "/tmp/x.md", "a/../../x.md", "./tasks.md", "", "a\\b.md", "tasks.md/"] {
            assert!(write_file(&dir, &kiro, rel, "x").is_err(), "{rel:?}");
        }
        assert!(!root.join("x.md").exists());
        assert!(!std::path::Path::new("/tmp/x.md").exists() || get(Path::new("/tmp/x.md")) != "x");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn writing_through_a_symlinked_folder_is_refused() {
        let (root, _, _) = world(&["api"]);
        let (dir, outside) = (root.join("spec"), root.join("outside"));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("docs/superpowers")).unwrap();
        let sp = named("superpowers");

        let err = write_file(&dir, &sp, "docs/superpowers/specs/x.md", "x");
        assert!(err.is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none(), "nothing reached the folder it points at");
        assert!(read_file(&dir, &sp, "docs/superpowers/specs/x.md").is_err());

        // A symlink standing for the file itself, and for the spec folder.
        std::fs::create_dir_all(dir.join("docs/superpowers.real")).unwrap();
        put(&outside.join("secret.md"), "secret");
        std::fs::remove_file(dir.join("docs/superpowers")).unwrap();
        std::fs::create_dir_all(dir.join("docs/superpowers")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), dir.join("docs/superpowers/plan.md")).unwrap();
        assert!(write_file(&dir, &sp, "docs/superpowers/plan.md", "x").is_err());
        assert!(read_file(&dir, &sp, "docs/superpowers/plan.md").is_err());
        assert_eq!(get(&outside.join("secret.md")), "secret");

        let linked = root.join("linked");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        assert!(write_file(&linked, &sp, "docs/superpowers/x.md", "x").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn writing_a_file_the_toolset_does_not_list_is_refused() {
        let (root, _, _) = world(&["api"]);
        let dir = root.join("spec");
        let openspec = named("openspec");
        assert!(write_file(&dir, &openspec, "notes.txt", "x").is_err());
        assert!(write_file(&dir, &openspec, "openspec/changes/x/tasks.txt", "x").is_err());
        assert!(!dir.exists(), "a refused write makes nothing");
        assert!(write_file(&dir, &openspec, "openspec/changes/x/tasks.md", "- [ ] a\n").is_ok());
        assert!(read_file(&dir, &openspec, "notes.txt").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_write_creates_missing_folders_and_replaces_whole_text() {
        let (root, _, _) = world(&["api"]);
        let dir = root.join("spec");
        let sp = named("superpowers");
        write_file(&dir, &sp, "docs/superpowers/plans/one.md", "first, longer text\n").unwrap();
        write_file(&dir, &sp, "docs/superpowers/plans/one.md", "second\n").unwrap();
        assert_eq!(read_file(&dir, &sp, "docs/superpowers/plans/one.md").unwrap(), "second\n");
        let left: Vec<_> = std::fs::read_dir(dir.join("docs/superpowers/plans")).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "no temporary file stays behind: {left:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn writes_at_once_never_leave_half_a_file() {
        let (root, _, _) = world(&["api"]);
        let dir = root.join("spec");
        let kiro = named("kiro");
        std::thread::scope(|s| {
            for n in 0..12 {
                let (dir, kiro) = (&dir, &kiro);
                s.spawn(move || write_file(dir, kiro, "tasks.md", &format!("{n}\n").repeat(5000)).unwrap());
            }
        });
        let text = read_file(&dir, &kiro, "tasks.md").unwrap();
        let first = text.lines().next().unwrap();
        assert!(text.lines().all(|l| l == first) && text.lines().count() == 5000, "one writer's whole text");
        assert_eq!(list_files(&dir, &kiro).unwrap(), vec!["tasks.md"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_listing_is_sorted_relative_and_leaves_out_links_and_what_the_toolset_does_not_name() {
        let (root, _, _) = world(&["api"]);
        let dir = root.join("spec");
        let openspec = named("openspec");
        put(&dir.join("openspec/changes/b/tasks.md"), "b");
        put(&dir.join("openspec/changes/a/proposal.md"), "a");
        put(&dir.join("openspec/changes/a/notes.txt"), "no");
        put(&dir.join(".claude/skills/x/SKILL.md"), "install output");
        std::os::unix::fs::symlink(root.join("tasks"), dir.join("openspec/changes/link")).unwrap();
        put(&root.join("elsewhere.md"), "e");
        std::os::unix::fs::symlink(root.join("elsewhere.md"), dir.join("openspec/changes/a/linked.md")).unwrap();

        assert_eq!(
            list_files(&dir, &openspec).unwrap(),
            vec!["openspec/changes/a/proposal.md", "openspec/changes/b/tasks.md"]
        );
        assert!(list_files(&dir.join("missing"), &openspec).unwrap().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn approved_drafts_move_into_spec() {
        let (root, state, task) = world(&["api"]);
        let task_root = root.join("tasks/T-1");
        put(&task_root.join(DRAFTS).join(APPROVED).join("requirements.md"), "## Requirements\n- R-1: x\n");
        put(&task_root.join(DRAFTS).join(APPROVED).join("api/tasks.md"), "- [ ] 1. Do it\n");

        assert!(move_old_spec(&state, &task).unwrap());

        let spec = task_root.join("spec");
        assert_eq!(get(&spec.join("requirements.md")), "## Requirements\n- R-1: x\n");
        assert_eq!(get(&spec.join("tasks.md")), "## api\n\n- [ ] 1. Do it\n");
        assert_eq!(state.config.task("t1").unwrap().toolset.as_deref(), Some("kiro"), "its files are Kiro's");
        assert!(task_root.join(DRAFTS).is_dir(), "the old folder stays for one release");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_newer_unapproved_draft_replaces_what_was_approved() {
        let (root, state, task) = world(&["api"]);
        let drafts = root.join("tasks/T-1").join(DRAFTS);
        put(&drafts.join(APPROVED).join("design.md"), "approved design");
        std::thread::sleep(std::time::Duration::from_millis(30));
        put(&drafts.join("design.md"), "newer draft");
        put(&drafts.join("requirements.md"), "only a draft");
        put(&drafts.join(APPROVED).join("requirements.md"), "approved after");

        move_old_spec(&state, &task).unwrap();

        let spec = root.join("tasks/T-1/spec");
        assert_eq!(get(&spec.join("design.md")), "newer draft");
        assert_eq!(get(&spec.join("requirements.md")), "approved after", "the approved one is newer here");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repository_copies_move_into_spec_when_nothing_was_approved() {
        let (root, state, task) = world(&["api", "web"]);
        let wt = |f: &str| root.join("tasks/T-1").join(f).join("specs/T-1");
        put(&wt("api").join("tasks.md"), "- [ ] 1. Endpoint\n");
        put(&wt("web").join("tasks.md"), "- [ ] 1. Screen\n");
        put(&wt("api").join("requirements.md"), "api requirements");
        put(&wt("web").join("requirements.md"), "web requirements");
        put(&wt("web").join("design.md"), "web design");

        assert!(move_old_spec(&state, &task).unwrap());

        let spec = root.join("tasks/T-1/spec");
        assert_eq!(get(&spec.join("tasks.md")), "## api\n\n- [ ] 1. Endpoint\n\n## web\n\n- [ ] 1. Screen\n");
        assert_eq!(get(&spec.join("requirements.md")), "api requirements", "the first repository's copy");
        assert_eq!(get(&spec.join("design.md")), "web design", "the first copy that has one");
        assert!(wt("api").join("tasks.md").is_file(), "the repository's copy is left alone");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_old_spec_md_becomes_requirements() {
        let (root, state, task) = world(&["api"]);
        put(&root.join("tasks/T-1/SPEC.md"), "# Old spec\n");

        assert!(move_old_spec(&state, &task).unwrap());

        assert_eq!(get(&root.join("tasks/T-1/spec/requirements.md")), "# Old spec\n");
        assert!(!root.join("tasks/T-1/spec/SPEC.md").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_task_with_nothing_old_gets_no_folder_and_none_is_made_over_an_existing_one() {
        let (root, state, task) = world(&["api"]);
        assert!(!move_old_spec(&state, &task).unwrap());
        assert!(!root.join("tasks/T-1/spec").exists());
        assert_eq!(state.config.task("t1").unwrap().toolset, None);

        put(&root.join("tasks/T-1/SPEC.md"), "old");
        put(&root.join("tasks/T-1/spec/requirements.md"), "already here");
        assert!(!move_old_spec(&state, &task).unwrap());
        assert_eq!(get(&root.join("tasks/T-1/spec/requirements.md")), "already here");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn moving_twice_changes_nothing() {
        let (root, state, task) = world(&["api", "web"]);
        let drafts = root.join("tasks/T-1").join(DRAFTS).join(APPROVED);
        put(&drafts.join("requirements.md"), "reqs");
        put(&drafts.join("api/tasks.md"), "- [ ] a\n");
        put(&drafts.join("web/tasks.md"), "- [ ] w\n");

        assert!(move_old_spec(&state, &task).unwrap());
        let spec = root.join("tasks/T-1/spec");
        let before = list_files(&spec, &named("kiro")).unwrap();
        // The agents have ticked a step since.
        put(&spec.join("tasks.md"), &get(&spec.join("tasks.md")).replacen("[ ]", "[x]", 1));
        let ticked = get(&spec.join("tasks.md"));

        assert!(!move_old_spec(&state, &task).unwrap());
        assert_eq!(list_files(&spec, &named("kiro")).unwrap(), before);
        assert_eq!(get(&spec.join("tasks.md")), ticked);
        let hidden: Vec<_> =
            std::fs::read_dir(root.join("tasks/T-1")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(!hidden.iter().any(|n| n.to_string_lossy().contains("moving")), "no staging folder: {hidden:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_task_with_no_folder_of_its_own_keeps_its_spec_where_it_already_is() {
        let (root, state, mut task) = world(&["api"]);
        task.root = root.join("tasks/T-1/api").to_string_lossy().to_string();
        let kept = root.join("specs/api/T-1");
        put(&kept.join("requirements.md"), "kept by the app");
        assert!(!move_old_spec(&state, &task).unwrap());

        // A repository that committed its spec: copied into the config folder.
        let (root2, state2, mut task2) = world(&["api"]);
        task2.root = root2.join("tasks/T-1/api").to_string_lossy().to_string();
        put(&root2.join("tasks/T-1/api/specs/T-1/design.md"), "committed");
        assert!(move_old_spec(&state2, &task2).unwrap());
        assert_eq!(get(&root2.join("specs/api/T-1/design.md")), "committed");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&root2).ok();
    }

    #[test]
    fn deleting_a_task_keeps_its_spec_in_the_config_folder() {
        let (root, state, task) = world(&["api"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("requirements.md"), "reqs");
        put(&spec.join("openspec/changes/x/tasks.md"), "- [ ] a\n");
        put(&root.join("elsewhere.md"), "not copied");
        std::os::unix::fs::symlink(root.join("elsewhere.md"), spec.join("link.md")).unwrap();

        archive_spec(&state, &task).unwrap();

        let kept = root.join("specs/T-1");
        assert_eq!(get(&kept.join("requirements.md")), "reqs");
        assert_eq!(get(&kept.join("openspec/changes/x/tasks.md")), "- [ ] a\n");
        assert!(!kept.join("link.md").exists(), "a link is not followed");
        assert!(spec.join("requirements.md").is_file(), "archiving copies; deleting is the caller's");

        remove_spec(&state, &task).unwrap();
        assert!(!spec.exists());

        // A later task with the same branch name does not overwrite it.
        put(&spec.join("requirements.md"), "reqs");
        archive_spec(&state, &task).unwrap();
        assert_eq!(get(&root.join("specs/T-1-2/requirements.md")), "reqs");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spec_already_in_the_config_folder_or_empty_is_not_archived_again() {
        let (root, state, task) = world(&["api"]);
        archive_spec(&state, &task).unwrap();
        std::fs::create_dir_all(root.join("tasks/T-1/spec/empty")).unwrap();
        archive_spec(&state, &task).unwrap();
        assert!(!root.join("specs").exists(), "nothing to keep, nothing made");

        let mut old = task.clone();
        old.root = root.join("tasks/T-1/api").to_string_lossy().to_string();
        put(&root.join("specs/api/T-1/requirements.md"), "kept");
        archive_spec(&state, &old).unwrap();
        assert!(!root.join("specs/T-1").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spec_too_deep_or_too_big_to_keep_stops_the_delete_and_is_left_alone() {
        let (root, state, task) = world(&["api"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("requirements.md"), "reqs");
        let deep = (0..=MOST_DEPTH + 1).map(|n| format!("d{n}")).collect::<Vec<_>>().join("/");
        put(&spec.join(&deep).join("x.md"), "deep");

        let err = archive_spec(&state, &task).unwrap_err().to_string();
        assert!(err.contains("nested too deeply"), "{err}");
        assert!(!root.join("specs").join("T-1").exists(), "no half-copy is kept");
        assert!(std::fs::read_dir(root.join("specs")).map(|mut d| d.next().is_none()).unwrap_or(true));
        assert!(spec.join(&deep).join("x.md").is_file(), "nothing was removed");

        std::fs::remove_dir_all(spec.join("d0")).unwrap();
        for n in 0..=MOST_FILES {
            put(&spec.join(format!("openspec/changes/c/{n}.md")), "");
        }
        let err = archive_spec(&state, &task).unwrap_err().to_string();
        assert!(err.contains("more than"), "{err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_installs_node_modules_is_not_kept_with_the_spec() {
        let (root, state, task) = world(&["api"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("requirements.md"), "reqs");
        put(&spec.join("node_modules/pkg/index.js"), "x");
        archive_spec(&state, &task).unwrap();
        assert!(root.join("specs/T-1/requirements.md").is_file());
        assert!(!root.join("specs/T-1/node_modules").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_leftover_folders_spec_is_kept_under_the_folders_name() {
        let (root, _, _) = world(&["api"]);
        let left = root.join("tasks/ACME-7");
        put(&left.join("spec/requirements.md"), "reqs");
        archive_leftover(&root, &left).unwrap();
        assert_eq!(get(&root.join("specs/ACME-7/requirements.md")), "reqs");
        std::fs::remove_dir_all(&root).ok();
    }
}
