//! Which branches a checkout's pull requests come from (PR-14): the task's
//! own, and the one its folder is on when an agent moved it to another.
//! Asked for the task's branch alone, a PR an agent opened from a branch of
//! its own was on GitHub while the task said "no PR".

use std::path::PathBuf;

use crate::config::Checkout;
use crate::error::Result;
use crate::git;
use crate::integrations::github::{GitHub, PullRequest};

use super::off_runtime;

/// The branches to ask GitHub about for `checkout`, the task's first.
/// Runs git, so not on the async workers.
pub(crate) fn branches_of(checkout: &Checkout, task_branch: &str) -> Vec<String> {
    let mut out = vec![task_branch.to_string()];
    out.extend(git::other_branch(&PathBuf::from(&checkout.path), task_branch, &checkout.base));
    out
}

/// [`branches_of`], off the async runtime.
pub(crate) async fn branches(checkout: &Checkout, task_branch: &str) -> Result<Vec<String>> {
    let (checkout, task_branch) = (checkout.clone(), task_branch.to_string());
    off_runtime(move || branches_of(&checkout, &task_branch)).await
}

/// Every pull request from `branches`, newest first.
pub(crate) async fn pulls(client: &GitHub, owner: &str, name: &str, branches: &[String]) -> Result<Vec<PullRequest>> {
    let found =
        futures_util::future::try_join_all(branches.iter().map(|b| client.pulls_for_branch(owner, name, b))).await?;
    Ok(newest_first(found.into_iter().flatten().collect()))
}

/// The newest open pull request from `branches`.
pub(crate) async fn open_pull(
    client: &GitHub,
    owner: &str,
    name: &str,
    branches: &[String],
) -> Result<Option<PullRequest>> {
    let found =
        futures_util::future::try_join_all(branches.iter().map(|b| client.pull_for_branch(owner, name, b))).await?;
    Ok(newest_first(found.into_iter().flatten().collect()).into_iter().next())
}

/// GitHub numbers a repository's pull requests in the order they were
/// opened, so the highest is the newest. One PR is listed once.
fn newest_first(mut prs: Vec<PullRequest>) -> Vec<PullRequest> {
    prs.sort_by_key(|p| std::cmp::Reverse(p.number));
    prs.dedup_by_key(|p| p.number);
    prs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pr(number: u64, head: &str) -> PullRequest {
        PullRequest {
            number,
            title: String::new(),
            state: "open".into(),
            draft: false,
            author: String::new(),
            head: head.into(),
            head_sha: String::new(),
            base: "main".into(),
            url: String::new(),
            mergeable_state: None,
            merged: false,
            comments: 0,
            review_comments: 0,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        git::run_for_tests(dir, args).map(|_| ()).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    }

    fn repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vl-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["-c", "user.email=a@b", "-c", "user.name=a", "commit", "-q", "--allow-empty", "-m", "one"]);
        dir
    }

    fn checkout(dir: &Path, base: &str) -> Checkout {
        Checkout {
            id: "c".into(),
            task_id: "t".into(),
            project_id: "p".into(),
            path: dir.to_string_lossy().into_owned(),
            base: base.into(),
            base_commit: None,
            push_lease: None,
            point_before_update: None,
            last_head: None,
        }
    }

    #[test]
    fn a_folder_an_agent_moved_to_its_own_branch_is_asked_about_that_branch_too() {
        let dir = repo();
        git(&dir, &["checkout", "-q", "-b", "fix/DT-22699-flight-offer-refused"]);
        assert_eq!(
            branches_of(&checkout(&dir, "main"), "DT-22303"),
            vec!["DT-22303".to_string(), "fix/DT-22699-flight-offer-refused".to_string()],
        );
    }

    #[test]
    fn a_folder_on_the_task_branch_is_asked_about_it_once() {
        let dir = repo();
        git(&dir, &["checkout", "-q", "-b", "DT-22303"]);
        assert_eq!(branches_of(&checkout(&dir, "main"), "DT-22303"), vec!["DT-22303".to_string()]);
    }

    #[test]
    fn a_folder_on_its_base_or_the_default_branch_or_detached_adds_nothing() {
        let dir = repo();
        git(&dir, &["branch", "development"]);
        git(&dir, &["checkout", "-q", "development"]);
        assert_eq!(branches_of(&checkout(&dir, "development"), "DT-1"), vec!["DT-1".to_string()]);
        git(&dir, &["checkout", "-q", "main"]);
        assert_eq!(branches_of(&checkout(&dir, "development"), "DT-1"), vec!["DT-1".to_string()]);
        git(&dir, &["checkout", "-q", "--detach"]);
        assert_eq!(branches_of(&checkout(&dir, "main"), "DT-1"), vec!["DT-1".to_string()]);
    }

    #[test]
    fn pull_requests_from_both_branches_are_listed_newest_first_and_once() {
        let prs = newest_first(vec![pr(731, "DT-22303"), pr(763, "fix/x"), pr(731, "DT-22303"), pr(12, "DT-22303")]);
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), vec![763, 731, 12]);
    }
}
