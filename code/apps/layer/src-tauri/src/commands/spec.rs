//! What the app keeps about a task's spec besides its files (§19): the last
//! check of each repository against the requirements (SPEC-27). The spec
//! itself is the files in `<task>/spec/` (`spec_copy.rs`); a check is a
//! verdict on the work at one commit, so it is kept beside them, where no
//! agent is told to look and no commit takes it.

use serde::{Deserialize, Serialize};

use crate::config::Task;
use crate::error::Result;
use crate::spec::glob;

use super::panes::agent_file_dir;
use super::spec_copy::write_under;
use super::AppState;

/// The task folder's last checks, one file per repository folder:
/// `.spec-checks/api.json`.
pub(crate) const CHECKS: &str = ".spec-checks";

/// The last check against the spec (SPEC-27).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    /// The commit it was checked at.
    pub sha: String,
    pub at: i64,
    pub results: Vec<Verdict>,
}

/// One requirement's answer: `met`, `not met` or `unclear`, and what shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub id: String,
    pub verdict: String,
    pub evidence: String,
}

/// Where the check of the repository checked out as `folder` is, in the task
/// folder. None for a task with no folder of its own, which keeps no checks,
/// and for a folder name that is no single path segment.
fn check_path(state: &AppState, task: &Task, folder: &str) -> Option<(std::path::PathBuf, String)> {
    let rel = format!("{CHECKS}/{folder}.json");
    (glob::is_safe_rel(&rel) && !folder.contains('/')).then_some(())?;
    Some((agent_file_dir(state, task)?, rel))
}

/// The last check of `folder`'s repository.
pub(crate) fn check_of(state: &AppState, task: &Task, folder: &str) -> Option<Check> {
    let (dir, rel) = check_path(state, task, folder)?;
    let path = dir.join(rel);
    // A link is not followed: an agent can plant one.
    std::fs::symlink_metadata(&path).ok().filter(|m| m.is_file())?;
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Keep `check` as `folder`'s last. A task with no folder of its own keeps
/// none: the check is still what the caller returns, just not remembered.
pub(crate) fn keep_check(state: &AppState, task: &Task, folder: &str, check: &Check) -> Result<()> {
    let Some((dir, rel)) = check_path(state, task, folder) else { return Ok(()) };
    write_under(&dir, &rel, &serde_json::to_string_pretty(check)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec_world::world;

    fn check() -> Check {
        Check {
            sha: "abc123".into(),
            at: 7,
            results: vec![Verdict { id: "R-1".into(), verdict: "met".into(), evidence: "api/x.rs:1".into() }],
        }
    }

    #[test]
    fn a_check_is_kept_in_the_task_folder_by_repository_and_read_back() {
        let (root, state, task) = world(&["api", "web"]);
        assert!(check_of(&state, &task, "api").is_none());

        keep_check(&state, &task, "api", &check()).unwrap();

        let back = check_of(&state, &task, "api").unwrap();
        assert_eq!((back.sha.as_str(), back.results[0].id.as_str()), ("abc123", "R-1"));
        assert!(root.join("tasks/T-1/.spec-checks/api.json").is_file());
        assert!(check_of(&state, &task, "web").is_none(), "one repository's check is not another's");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_check_is_never_written_through_a_link() {
        let (root, state, task) = world(&["api"]);
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("tasks/T-1/.spec-checks")).unwrap();

        assert!(keep_check(&state, &task, "api", &check()).is_err());

        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        assert!(check_of(&state, &task, "../escape").is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
