//! Commit spec (§19): the task's `spec/` working copy, put into each
//! repository's worktree and committed there on the task's branch, per that
//! repository's `spec_commit`.
//!
//! `spec/` mirrors a repository's root, so a file goes to the same path
//! there; Kiro's, which are laid out flat, go into the repository's spec
//! folder under the task's name. A repository's own files are never deleted:
//! this writes the paths the spec has, and commits those paths and no others
//! (`git::commit_only`), so an agent's staged work is left as it is.

use std::path::Path;

use tauri::AppHandle;

use crate::config::{Checkout, Project, SpecCommit, Task};
use crate::error::{Error, Result};
use crate::git;
use crate::spec::owned::own_part;
use crate::spec::toolset::Toolset;
use crate::spec::{self, glob};

use super::diff::RepoResult;
use super::spec_copy::{list_files, read_file, spec_dir, toolset_of, write_under};
use super::AppState;

/// The one toolset whose files are not laid out as a repository root.
const KIRO: &str = "kiro";

/// Where `rel`, a file of `task`'s spec, goes in `project`'s repository:
/// where it is, or for Kiro under `<spec_folder or specs>/<task>/`. None
/// when `rel` or the folder would leave the repository.
pub(crate) fn target_path(t: &Toolset, task: &Task, project: &Project, rel: &str) -> Option<String> {
    if !glob::is_safe_rel(rel) {
        return None;
    }
    let to = if t.id == KIRO {
        let base = project.spec_folder.as_deref().unwrap_or("specs");
        glob::is_safe_rel(base).then(|| format!("{base}/{}/{rel}", spec::folder_name(&task.branch)))?
    } else {
        rel.to_string()
    };
    // A spec folder of `.git/hooks` would be a way to run something.
    (to.split('/').next() != Some(".git")).then_some(to)
}

/// One repository of the task, and what names it in a step file.
struct Repo {
    checkout: Checkout,
    project: Project,
    folder: String,
}

fn repositories(state: &AppState, task: &Task) -> Vec<Repo> {
    state
        .config
        .checkouts_of(&task.id)
        .into_iter()
        .filter_map(|checkout| {
            let project = state.config.project(&checkout.project_id).ok()?;
            let folder = Path::new(&checkout.path).file_name()?.to_string_lossy().to_string();
            Some(Repo { checkout, project, folder })
        })
        .collect()
}

/// What Commit spec writes into `repo`: each file of the spec with the path
/// it goes to. `own` filters the step files down to the repository's steps.
fn would_write(
    spec: &Path,
    t: &Toolset,
    task: &Task,
    repo: &Repo,
    folders: &[String],
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for rel in list_files(spec, t)? {
        let mut text = read_file(spec, t, &rel)?;
        if repo.project.spec_commit == SpecCommit::Own && t.steps.iter().any(|g| glob::matches(g, &rel)) {
            text = own_part(&text, &repo.folder, folders);
        }
        let to = target_path(t, task, &repo.project, &rel).ok_or_else(|| {
            Error::Other(format!("{rel} has no place inside {}: check its spec folder", repo.project.name))
        })?;
        out.push((to, text));
    }
    Ok(out)
}

/// The sentence for a commit; `git::commit_only` writes it as the repository
/// writes its own (`docs: update the spec for ACME-1`).
fn message(task: &Task) -> String {
    format!("Update the spec for {}.", task.issue_key.as_deref().unwrap_or(&task.branch))
}

/// Write the spec into one repository and commit it there. What it says is a
/// row's detail.
fn commit_in(spec: &Path, t: &Toolset, task: &Task, repo: &Repo, folders: &[String]) -> Result<String> {
    if repo.project.spec_commit == SpecCommit::None {
        return Ok("the spec stays in Layer here (spec_commit is none)".into());
    }
    let files = would_write(spec, t, task, repo, folders)?;
    if files.is_empty() {
        return Ok("the spec has no files yet".into());
    }
    let worktree = Path::new(&repo.checkout.path);
    for (rel, text) in &files {
        let same = std::fs::symlink_metadata(worktree.join(rel)).is_ok_and(|m| m.is_file())
            && std::fs::read_to_string(worktree.join(rel)).is_ok_and(|now| now == *text);
        if !same {
            write_under(worktree, rel, text)?;
        }
    }
    let paths: Vec<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
    Ok(match git::commit_only(worktree, &paths, &message(task))? {
        Some(sha) => format!("committed {}", sha.get(..7).unwrap_or(&sha)),
        None => "nothing changed".into(),
    })
}

/// Commit the task's spec into each repository (`only`: those whose folder
/// is named, else all), per its `spec_commit`. A row per repository, so one
/// that failed is named with why and the others keep their commit.
pub(crate) fn commit_spec(state: &AppState, task: &Task, only: Option<&[String]>) -> Vec<RepoResult> {
    let t = toolset_of(state, task);
    let spec = spec_dir(state, task);
    let repos = repositories(state, task);
    let folders: Vec<String> = repos.iter().map(|r| r.folder.clone()).collect();
    repos
        .iter()
        .filter(|r| only.is_none_or(|names| names.contains(&r.folder)))
        .map(|repo| {
            let done = commit_in(&spec, &t, task, repo, &folders);
            RepoResult {
                checkout_id: repo.checkout.id.clone(),
                repo: repo.project.name.clone(),
                ok: done.is_ok(),
                detail: done.unwrap_or_else(|e| e.to_string()),
            }
        })
        .collect()
}

/// The folders of the repositories whose last commit does not hold what
/// Commit spec would write now. One whose spec cannot be read is named too:
/// Commit spec is where that is said. `spec_commit = none` is never changed.
pub(crate) fn changed_since_commit(state: &AppState, task: &Task) -> Vec<String> {
    let t = toolset_of(state, task);
    let spec = spec_dir(state, task);
    let repos = repositories(state, task);
    let folders: Vec<String> = repos.iter().map(|r| r.folder.clone()).collect();
    repos
        .iter()
        .filter(|r| r.project.spec_commit != SpecCommit::None)
        .filter(|repo| match would_write(&spec, &t, task, repo, &folders) {
            Ok(files) => files
                .iter()
                .any(|(rel, text)| git::committed_text(Path::new(&repo.checkout.path), rel).as_deref() != Some(text)),
            Err(_) => true,
        })
        .map(|r| r.folder.clone())
        .collect()
}

/// Open pull requests sends the spec with the code: Commit spec in every
/// repository the user did not leave out (`skip`, checkout ids) whose
/// committed copy is behind the working copy.
pub(crate) fn commit_changed(state: &AppState, task: &Task, skip: &[String]) -> Vec<RepoResult> {
    let behind = changed_since_commit(state, task);
    let wanted: Vec<String> = repositories(state, task)
        .into_iter()
        .filter(|r| behind.contains(&r.folder) && !skip.contains(&r.checkout.id))
        .map(|r| r.folder)
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    commit_spec(state, task, Some(&wanted))
}

/// Commit spec: the Spec tab's button.
#[tauri::command]
pub async fn commit_spec_cmd(app: AppHandle, task_id: String, repos: Option<Vec<String>>) -> Result<Vec<RepoResult>> {
    super::blocking(app, move |state| {
        let task = state.config.task(&task_id)?;
        super::pr_task::not_a_review(&task, "given a spec of its own")?;
        Ok(commit_spec(state, &task, repos.as_deref()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::commands::spec_world::{get, put, world};
    use crate::config::SpecCommit;
    use crate::git;
    use crate::spec::toolset;

    /// A task `ACME-1-x` (ticket ACME-1) of `folders`, each a git repository
    /// with one commit, whose spec is `kit`'s.
    fn repos(folders: &[&str], kit: &str) -> (PathBuf, AppState, Task) {
        let (root, state, mut task) = world(folders);
        task.branch = "ACME-1-x".into();
        task.issue_key = Some("ACME-1".into());
        task.toolset = Some(kit.into());
        for folder in folders {
            let dir = worktree(&root, folder);
            for args in [
                &["init", "-q", "-b", "main"][..],
                &["config", "user.email", "t@villain.local"],
                &["config", "user.name", "T"],
            ] {
                git::run_for_tests(&dir, args).unwrap();
            }
            put(&dir.join("README.md"), "# readme\n");
            git::run_for_tests(&dir, &["add", "-A"]).unwrap();
            git::run_for_tests(&dir, &["commit", "-q", "-m", "init"]).unwrap();
        }
        (root, state, task)
    }

    fn worktree(root: &Path, folder: &str) -> PathBuf {
        root.join("tasks/T-1").join(folder)
    }

    fn spec_file(root: &Path, rel: &str, text: &str) {
        put(&root.join("tasks/T-1/spec").join(rel), text);
    }

    fn mode(state: &AppState, folder: &str, commit: SpecCommit) {
        state
            .config
            .update(|c| {
                if let Some(p) = c.projects.iter_mut().find(|p| p.id == format!("p-{folder}")) {
                    p.spec_commit = commit;
                }
            })
            .unwrap();
    }

    fn log(dir: &Path) -> Vec<String> {
        let out = git::run_for_tests(dir, &["log", "--format=%s"]).unwrap();
        out.lines().map(String::from).collect()
    }

    /// The files the last commit touched.
    fn touched(dir: &Path) -> Vec<String> {
        let out = git::run_for_tests(dir, &["show", "--name-only", "--format=", "HEAD"]).unwrap();
        out.lines().map(String::from).collect()
    }

    fn by_repo<'a>(rows: &'a [RepoResult], repo: &str) -> &'a RepoResult {
        rows.iter().find(|r| r.repo == repo).unwrap_or_else(|| panic!("no row for {repo}: {rows:?}"))
    }

    const TASKS: &str =
        "# Tasks\n\nShared intro.\n\n## api\n\n- [ ] 1.1 Add the endpoint\n\n## web\n\n- [ ] 2.1 Call it\n";

    #[test]
    fn whole_copies_every_spec_file_to_its_mirrored_path() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");
        spec_file(&root, "openspec/changes/a/proposal.md", "Why\n");
        spec_file(&root, "scratch/notes.txt", "not a spec file\n");

        let rows = commit_spec(&state, &task, None);

        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        for folder in ["api", "web"] {
            let dir = worktree(&root, folder);
            assert_eq!(get(&dir.join("openspec/changes/a/tasks.md")), "- [ ] 1.1 Do it\n");
            assert_eq!(get(&dir.join("openspec/changes/a/proposal.md")), "Why\n");
            assert!(!dir.join("scratch").exists(), "only what the toolset lists is copied");
            assert_eq!(log(&dir)[0], "Update the spec for ACME-1.");
            assert_eq!(touched(&dir), ["openspec/changes/a/proposal.md", "openspec/changes/a/tasks.md"]);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn kiro_files_go_under_the_tasks_spec_folder() {
        let (root, state, task) = repos(&["api", "web"], "kiro");
        state
            .config
            .update(|c| {
                if let Some(p) = c.projects.iter_mut().find(|p| p.id == "p-web") {
                    p.spec_folder = Some("docs/specs".into());
                }
            })
            .unwrap();
        spec_file(&root, "requirements.md", "## Requirements\n");
        spec_file(&root, "tasks.md", "- [ ] 1. Do it\n");

        let rows = commit_spec(&state, &task, None);

        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        let api = worktree(&root, "api");
        assert_eq!(get(&api.join("specs/ACME-1-x/requirements.md")), "## Requirements\n");
        assert_eq!(touched(&api), ["specs/ACME-1-x/requirements.md", "specs/ACME-1-x/tasks.md"]);
        assert!(!api.join("requirements.md").exists());
        let web = worktree(&root, "web");
        assert_eq!(get(&web.join("docs/specs/ACME-1-x/tasks.md")), "- [ ] 1. Do it\n", "spec_folder moves Kiro's");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_target_path_only_moves_kiros_files_and_never_leaves_the_repository() {
        let (root, state, task) = repos(&["api"], "kiro");
        let mut project = state.config.project("p-api").unwrap();
        let kiro = toolset::find("kiro", &[]).unwrap();
        let openspec = toolset::find("openspec", &[]).unwrap();
        assert_eq!(target_path(&kiro, &task, &project, "tasks.md").as_deref(), Some("specs/ACME-1-x/tasks.md"));
        assert_eq!(
            target_path(&openspec, &task, &project, "openspec/specs/a.md").as_deref(),
            Some("openspec/specs/a.md"),
            "a mirrored toolset is not moved, whatever spec_folder says"
        );
        project.spec_folder = Some("../out".into());
        assert_eq!(target_path(&kiro, &task, &project, "tasks.md"), None);
        project.spec_folder = Some(".git/hooks".into());
        assert_eq!(target_path(&kiro, &task, &project, "tasks.md"), None, "never into git's own folder");
        project.spec_folder = None;
        assert_eq!(target_path(&kiro, &task, &project, "../tasks.md"), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn own_keeps_only_this_repositorys_steps() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        mode(&state, "api", SpecCommit::Own);
        mode(&state, "web", SpecCommit::Own);
        spec_file(&root, "openspec/changes/a/tasks.md", TASKS);
        spec_file(&root, "openspec/changes/a/proposal.md", "## web\nNot a step file, so whole.\n");

        let rows = commit_spec(&state, &task, None);

        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        let api = get(&worktree(&root, "api").join("openspec/changes/a/tasks.md"));
        assert!(api.contains("Shared intro.") && api.contains("Add the endpoint"), "{api}");
        assert!(!api.contains("Call it"), "{api}");
        let web = get(&worktree(&root, "web").join("openspec/changes/a/tasks.md"));
        assert!(web.contains("Shared intro.") && web.contains("Call it"), "{web}");
        assert!(!web.contains("Add the endpoint"), "{web}");
        let proposal = "## web\nNot a step file, so whole.\n";
        for folder in ["api", "web"] {
            assert_eq!(get(&worktree(&root, folder).join("openspec/changes/a/proposal.md")), proposal);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn none_writes_nothing() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        mode(&state, "api", SpecCommit::None);
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        let rows = commit_spec(&state, &task, None);

        let api = worktree(&root, "api");
        assert!(by_repo(&rows, "api").ok, "{rows:?}");
        assert!(!api.join("openspec").exists());
        assert_eq!(log(&api), ["init"], "no commit");
        assert!(worktree(&root, "web").join("openspec/changes/a/tasks.md").is_file());
        assert!(changed_since_commit(&state, &task).iter().all(|f| f != "api"), "never changed");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn existing_repository_spec_files_are_never_deleted_or_touched() {
        let (root, state, task) = repos(&["api"], "openspec");
        let api = worktree(&root, "api");
        put(&api.join("openspec/specs/auth/spec.md"), "# Auth\nolder, written by hand\n");
        put(&api.join("docs/superpowers/plans/old.md"), "- [ ] an older plan\n");
        git::run_for_tests(&api, &["add", "-A"]).unwrap();
        git::run_for_tests(&api, &["commit", "-q", "-m", "older specs"]).unwrap();
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        let rows = commit_spec(&state, &task, None);

        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        assert_eq!(get(&api.join("openspec/specs/auth/spec.md")), "# Auth\nolder, written by hand\n");
        assert_eq!(get(&api.join("docs/superpowers/plans/old.md")), "- [ ] an older plan\n");
        let stat = git::run_for_tests(&api, &["show", "--stat", "--format=", "HEAD"]).unwrap();
        assert!(stat.contains("openspec/changes/a/tasks.md"), "{stat}");
        assert_eq!(stat.lines().filter(|l| l.contains('|')).count(), 1, "only the spec path: {stat}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_spec_paths_are_committed_while_an_agent_has_changes_staged() {
        let (root, state, task) = repos(&["api"], "openspec");
        let api = worktree(&root, "api");
        put(&api.join("README.md"), "# readme\nan agent's edit\n");
        git::run_for_tests(&api, &["add", "README.md"]).unwrap();
        put(&api.join("src/new.rs"), "fn main() {}\n");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        let rows = commit_spec(&state, &task, None);

        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        assert_eq!(touched(&api), ["openspec/changes/a/tasks.md"]);
        let status = git::run_for_tests(&api, &["status", "--porcelain", "--untracked-files=normal"]).unwrap();
        assert!(status.contains("M  README.md"), "the agent's staged edit stays staged: {status}");
        assert!(status.contains("?? src/"), "the agent's new file is still untracked: {status}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_repository_that_fails_is_named_and_the_others_commit() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, worktree(&root, "web").join("openspec")).unwrap();
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        let rows = commit_spec(&state, &task, None);

        let web = by_repo(&rows, "web");
        assert!(!web.ok && web.detail.contains("link"), "{web:?}");
        assert_eq!(web.checkout_id, "c-web");
        assert!(by_repo(&rows, "api").ok, "{rows:?}");
        assert_eq!(touched(&worktree(&root, "api")), ["openspec/changes/a/tasks.md"]);
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing written through the link");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_conventional_repository_gets_a_conventional_message() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        put(&worktree(&root, "api").join("commitlint.config.js"), "module.exports = {};\n");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        commit_spec(&state, &task, None);

        assert_eq!(log(&worktree(&root, "api"))[0], "docs: update the spec for ACME-1");
        assert_eq!(log(&worktree(&root, "web"))[0], "Update the spec for ACME-1.");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nothing_changed_means_no_commit() {
        let (root, state, task) = repos(&["api"], "openspec");
        let api = worktree(&root, "api");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");
        assert_eq!(changed_since_commit(&state, &task), ["api"]);
        commit_spec(&state, &task, None);
        let head = git::head_commit(&api).unwrap();
        assert!(changed_since_commit(&state, &task).is_empty());

        let rows = commit_spec(&state, &task, None);

        assert!(rows.iter().all(|r| r.ok), "{rows:?}");
        assert_eq!(git::head_commit(&api).unwrap(), head, "no second commit");

        spec_file(&root, "openspec/changes/a/tasks.md", "- [x] 1.1 Do it\n");
        assert_eq!(changed_since_commit(&state, &task), ["api"], "a tick is a change");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_the_named_repositories_are_committed() {
        let (root, state, task) = repos(&["api", "web"], "openspec");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");

        let rows = commit_spec(&state, &task, Some(&["web".to_string()]));

        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].repo, "web");
        assert_eq!(log(&worktree(&root, "api")), ["init"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn before_pull_requests_only_repositories_that_are_behind_and_not_left_out_are_committed() {
        let (root, state, task) = repos(&["api", "web", "docs"], "openspec");
        spec_file(&root, "openspec/changes/a/tasks.md", "- [ ] 1.1 Do it\n");
        commit_spec(&state, &task, Some(&["api".to_string()]));

        let rows = commit_changed(&state, &task, &["c-docs".to_string()]);

        assert_eq!(rows.len(), 1, "api is current and docs was left out: {rows:?}");
        assert_eq!(rows[0].repo, "web");
        assert_eq!(log(&worktree(&root, "docs")), ["init"]);
        assert!(commit_changed(&state, &task, &["c-docs".to_string()]).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
