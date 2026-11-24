//! What the Spec tab is shown of a task's spec (SPEC-8), and the one command
//! that writes a file of it: the toolset's files with their steps, how far
//! each repository is, what was last committed, and how the install stands.

use serde::Serialize;

use crate::config::{SpecCommit, Task};
use crate::error::Result;
use crate::git;
use crate::spec::glob;
use crate::spec::owned::{owned_steps, OwnedStep};
use crate::spec::toolset::{Extra, Toolset};

use super::spec::{check_of, Check};
use super::spec_commit::{changed_since_commit, repositories};
use super::spec_copy::{list_files, read_all, read_file, spec_dir, toolset_of};
use super::spec_install::{state_of, InstallState};
use super::spec_work::{remove_spec, with_spec_moved, write_checked};
use super::AppState;

/// The toolset as the tab shows it: its name, and which extras it has. Not the
/// manifest: nothing in the install steps or the agents' paragraph is for the UI.
#[derive(Debug, Serialize)]
pub struct ToolsetInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub extras: Vec<Extra>,
}

/// One file of the spec. `steps` are those of a file the toolset keeps steps in.
#[derive(Debug, Serialize)]
pub struct SpecFileView {
    pub path: String,
    pub text: String,
    pub steps: Vec<OwnedStep>,
}

#[derive(Debug, Serialize)]
pub struct CheckView {
    #[serde(flatten)]
    pub check: Check,
    /// The branch has moved on since.
    pub stale: bool,
}

/// One repository's share: how far its steps are, and where Commit spec stands.
#[derive(Debug, Serialize)]
pub struct RepoSpecView {
    pub checkout_id: String,
    pub repo: String,
    pub folder: String,
    pub commit: SpecCommit,
    /// Steps ticked of all those the repository owns, or that belong to everyone.
    pub done: u32,
    pub total: u32,
    /// What Commit spec would write now is not what its last commit holds.
    pub changed_since_commit: bool,
    /// The last check against the spec (Kiro's extra).
    pub check: Option<CheckView>,
}

/// A task's spec as the Spec tab shows it.
#[derive(Debug, Serialize)]
pub struct SpecView {
    pub toolset: ToolsetInfo,
    /// The working copy: where agents and the user edit.
    pub dir: String,
    pub files: Vec<SpecFileView>,
    /// Files the toolset keeps that are not UTF-8 text, so they are not shown or
    /// committed. Named so that a file missing from the tree is explained.
    pub unreadable: Vec<String>,
    pub repos: Vec<RepoSpecView>,
    pub install: Option<InstallState>,
}

fn holds_steps(t: &Toolset, rel: &str) -> bool {
    t.steps.iter().any(|g| glob::matches(g, rel))
}

/// The text of each file of steps the spec has, with its path: what an epic
/// plans its sub-tasks from and counts them by. A file that cannot be read is
/// left out.
pub(crate) fn step_texts(state: &AppState, task: &Task) -> Vec<(String, String)> {
    let task = with_spec_moved(state, task).unwrap_or_else(|_| task.clone());
    let (t, dir) = (toolset_of(state, &task), spec_dir(state, &task));
    let files = list_files(&dir, &t).unwrap_or_default();
    files
        .into_iter()
        .filter(|rel| holds_steps(&t, rel))
        .filter_map(|rel| Some((rel.clone(), read_file(&dir, &t, &rel).ok()?)))
        .collect()
}

/// The steps of one repository among `steps`: its own and everyone's.
fn mine<'a>(steps: &'a [OwnedStep], folder: &'a str) -> impl Iterator<Item = &'a OwnedStep> {
    steps.iter().filter(move |s| s.repos.is_empty() || s.repos.iter().any(|r| r.eq_ignore_ascii_case(folder)))
}

/// Runs git, for whether a check is current and what is committed.
pub(crate) fn view(state: &AppState, task: &Task) -> Result<SpecView> {
    let task = with_spec_moved(state, task)?;
    let (t, dir) = (toolset_of(state, &task), spec_dir(state, &task));
    let repos = repositories(state, &task);
    let folders: Vec<String> = repos.iter().map(|r| r.folder.clone()).collect();
    let (texts, unreadable) = read_all(&dir, &t)?;
    let files: Vec<SpecFileView> = texts
        .into_iter()
        .map(|(path, text)| {
            let steps = if holds_steps(&t, &path) { owned_steps(&text, &folders) } else { Vec::new() };
            SpecFileView { path, text, steps }
        })
        .collect();
    let behind = changed_since_commit(state, &task);
    let repos = repos
        .iter()
        .map(|r| {
            let own: Vec<&OwnedStep> = files.iter().flat_map(|f| mine(&f.steps, &r.folder)).collect();
            let head = git::head_commit(std::path::Path::new(&r.checkout.path)).ok();
            RepoSpecView {
                checkout_id: r.checkout.id.clone(),
                repo: r.project.name.clone(),
                folder: r.folder.clone(),
                commit: r.project.spec_commit,
                done: own.iter().filter(|s| s.done).count() as u32,
                total: own.len() as u32,
                changed_since_commit: behind.contains(&r.folder),
                check: check_of(state, &task, &r.folder)
                    .map(|check| CheckView { stale: head.as_deref() != Some(check.sha.as_str()), check }),
            }
        })
        .collect();
    Ok(SpecView {
        toolset: ToolsetInfo {
            id: t.id.clone(),
            name: t.name.clone(),
            version: t.version.clone(),
            extras: t.extras.clone(),
        },
        dir: dir.display().to_string(),
        files,
        unreadable,
        repos,
        install: state_of(state, &task),
    })
}

/// The task's spec: its files with their steps. A spec kept the way earlier
/// builds did is moved into place first.
#[tauri::command]
pub async fn read_spec(app: tauri::AppHandle, task_id: String) -> Result<SpecView> {
    super::blocking(app, move |state| view(state, &state.config.task(&task_id)?)).await
}

/// Replace one file of the spec with `text`, as an agent's `spec_write` does,
/// through the same lock and the same refusals: a path outside the spec
/// folder, a file the toolset does not keep, a link. An editor's save sends
/// `base`, the text it began from, and is not written (false) when the file
/// holds something else by then: an agent wrote it (SPEC-37). `force` is Save
/// mine, which writes over it.
#[tauri::command]
pub async fn write_spec_file(
    app: tauri::AppHandle,
    task_id: String,
    path: String,
    text: String,
    base: Option<String>,
    force: Option<bool>,
) -> Result<bool> {
    super::blocking(app, move |state| {
        write_checked(state, &state.config.task(&task_id)?, &path, &text, base.as_deref(), force.unwrap_or(false))
    })
    .await
}

/// Delete one file of the spec, for the Undo of a draft that made it: the
/// same refusals as a write, and nothing when it is already gone.
#[tauri::command]
pub async fn remove_spec_file(app: tauri::AppHandle, task_id: String, path: String) -> Result<()> {
    super::blocking(app, move |state| remove_spec(state, &state.config.task(&task_id)?, &path)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec_work::write_spec;
    use crate::commands::spec_world::{get, put, world};

    #[test]
    fn read_spec_lists_the_toolsets_files_with_their_steps() {
        let (root, state, task) = world(&["api", "web"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("requirements.md"), "## Requirements\n- R-1 [api]: x\n");
        put(&spec.join("tasks.md"), "# api\n\n## Tasks\n- [x] 1. a\n- [ ] 2. b\n\n# web\n\n## Tasks\n- [ ] 1. c\n");
        put(&spec.join("notes.txt"), "not a file Kiro keeps");

        let v = view(&state, &task).unwrap();

        assert_eq!((v.toolset.id.as_str(), v.dir.as_str()), ("kiro", spec.to_str().unwrap()));
        assert_eq!(v.toolset.extras, [Extra::Draft, Extra::Check, Extra::Questions]);
        let paths: Vec<&str> = v.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["requirements.md", "tasks.md"]);
        assert!(v.files[0].steps.is_empty() && v.files[0].text.contains("R-1"));
        assert_eq!(v.files[1].steps.len(), 3);
        let api = &v.repos[0];
        assert_eq!((api.folder.as_str(), api.done, api.total), ("api", 1, 2));
        let web = &v.repos[1];
        assert_eq!((web.folder.as_str(), web.done, web.total), ("web", 0, 1));
        assert!(api.changed_since_commit, "nothing was committed");
        assert!(v.install.is_none(), "nothing was installed");
        let json = serde_json::to_value(&v).unwrap();
        assert!(json["toolset"].get("install").is_none() && json["toolset"].get("agents_md").is_none());
        assert_eq!(json["repos"][0]["commit"], "whole");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_that_is_not_utf8_is_named_and_the_rest_of_the_spec_is_still_shown() {
        let (root, state, task) = world(&["api"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("requirements.md"), "## Requirements\n");
        std::fs::write(spec.join("design.md"), [0xff, 0xfe, 0x00]).unwrap();

        let v = view(&state, &task).unwrap();

        let paths: Vec<&str> = v.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["requirements.md"]);
        assert_eq!(v.unreadable, ["design.md"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_spec_moves_a_spec_kept_the_old_way_first() {
        let (root, state, task) = world(&["api"]);
        put(&root.join("tasks/T-1/.spec-drafts/approved/requirements.md"), "# Old\n");

        let v = view(&state, &task).unwrap();

        assert_eq!(v.files.len(), 1);
        assert_eq!(get(&root.join("tasks/T-1/spec/requirements.md")), "# Old\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_tab_writes_a_file_as_an_agent_does_and_is_refused_as_one_is() {
        let (root, state, task) = world(&["api"]);

        write_spec(&state, &task, "design.md", "# D\n").unwrap();
        assert_eq!(get(&root.join("tasks/T-1/spec/design.md")), "# D\n");

        assert!(write_spec(&state, &task, "../escape.md", "x").is_err());
        assert!(write_spec(&state, &task, "notes.txt", "x").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_tab_removes_a_file_a_draft_made_and_nothing_else() {
        let (root, state, task) = world(&["api"]);
        let spec = root.join("tasks/T-1/spec");
        put(&spec.join("design.md"), "# D\n");
        put(&spec.join("notes.txt"), "not Kiro's");

        remove_spec(&state, &task, "design.md").unwrap();
        assert!(!spec.join("design.md").exists(), "the drafted file is gone, not emptied");
        remove_spec(&state, &task, "design.md").expect("already gone is what was asked for");

        assert!(remove_spec(&state, &task, "notes.txt").is_err(), "not a file Kiro keeps");
        assert!(remove_spec(&state, &task, "../escape.md").is_err());
        assert!(spec.join("notes.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_install_shows_as_its_record_says() {
        let (root, state, task) = world(&["api"]);
        put(
            &root.join("tasks/T-1/.spec-install.json"),
            r#"{"toolset":"openspec","version":"1.14.3","ok":false,"error":"no npx","log_tail":"","written":[]}"#,
        );

        let v = view(&state, &task).unwrap();

        let install = v.install.unwrap();
        assert_eq!(
            (install.toolset.as_str(), install.ok, install.error.as_deref()),
            ("openspec", false, Some("no npx"))
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
