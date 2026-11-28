//! Which branches a checkout's pull requests come from (PR-14): the task's
//! own, the current topic, and exact historical tips its worktree recorded.

use std::path::{Path, PathBuf};

use crate::config::{AppConfig, Checkout, GithubConfig, Task};
use crate::error::{Error, Result};
use crate::git;
use crate::integrations::github::{BranchPulls, GitHub, PullRequest};

/// Config-only snapshot. Take it under the config read lock, then release
/// the lock before asking Git about the worktree.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HistoryScope {
    since: i64,
    configured_default: Option<String>,
    excluded: Vec<String>,
}

impl HistoryScope {
    pub(crate) fn for_checkout(config: &AppConfig, task: &Task, checkout: &Checkout) -> Self {
        let project = config.projects.iter().find(|p| p.id == checkout.project_id);
        let configured_default = project.map(|p| &p.default_branch).filter(|b| !b.is_empty()).cloned();
        let mut excluded = Vec::new();
        for other in &config.checkouts {
            if other.task_id == task.id {
                continue;
            }
            let same_repo = other.project_id == checkout.project_id
                || project.is_some_and(|p| {
                    config.projects.iter().find(|q| q.id == other.project_id).is_some_and(|q| {
                        p.repository_id.as_ref().is_some_and(|id| q.repository_id.as_ref() == Some(id))
                            || p.remote.as_ref().zip(q.remote.as_ref()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                    })
                });
            if same_repo {
                if let Some(task) = config.tasks.iter().find(|t| t.id == other.task_id) {
                    excluded.push(task.branch.clone());
                }
            }
        }
        excluded.sort();
        excluded.dedup();
        Self { since: task.created_at.timestamp(), configured_default, excluded }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchCandidate {
    pub branch: String,
    /// `None` allows any PR head commit on a direct branch.
    pub exact_shas: Option<Vec<String>>,
}

#[derive(Debug)]
pub(crate) struct BranchEvidence {
    pub candidates: Vec<BranchCandidate>,
    pub errors: Vec<Error>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct BranchStamp {
    candidates: Vec<BranchCandidate>,
    complete: bool,
}

impl BranchEvidence {
    pub(super) fn stamp(&self) -> BranchStamp {
        BranchStamp { candidates: self.candidates.clone(), complete: self.errors.is_empty() }
    }
}

/// Resolve the checkout's repo only on the configured forge. An SSH port
/// belongs to a different transport; an HTTP(S) port must match the web site.
/// Runs Git; call on the blocking pool.
pub(crate) fn configured_checkout_repo(dir: &Path, cfg: &GithubConfig) -> Result<(String, String)> {
    let origin = git::checkout_origin(dir)?;
    let web = forge_url(&cfg.web_url)
        .ok_or_else(|| Error::Other("The configured GitHub web address is not valid.".into()))?;
    let api = forge_url(&cfg.api_url)
        .ok_or_else(|| Error::Other("The configured GitHub API address is not valid.".into()))?;
    let host = web.host_str().unwrap_or_default();
    let https = |u: &reqwest::Url| u.scheme() == "https" && u.port_or_known_default() == Some(443);
    let compatible = match host {
        "github.com" => https(&web) && https(&api) && api.host_str() == Some("api.github.com"),
        "api.github.com" | "ssh.github.com" => false,
        host if host.ends_with(".ghe.com") => {
            https(&web) && https(&api) && api.host_str() == Some(format!("api.{host}").as_str())
        }
        _ => web.origin() == api.origin(),
    };
    if !compatible {
        return Err(Error::Other("The configured GitHub API and web addresses name different sites.".into()));
    }
    let github_ssh = host == "github.com" && origin.host == "ssh.github.com" && origin.ssh_port == Some(443);
    if (origin.host != host && !github_ssh)
        || origin.web_port.is_some_and(|port| Some(port) != web.port_or_known_default())
    {
        return Err(Error::Other("The checkout's origin does not match the configured GitHub site.".into()));
    }
    Ok((origin.owner, origin.repo))
}

fn forge_url(raw: &str) -> Option<reqwest::Url> {
    reqwest::Url::parse(raw).ok().filter(|u| {
        matches!(u.scheme(), "https" | "http")
            && u.host().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

/// Runs Git; call on the blocking pool.
pub(crate) fn branch_evidence(checkout: &Checkout, task_branch: &str, scope: &HistoryScope) -> BranchEvidence {
    let dir = PathBuf::from(&checkout.path);
    let mut candidates = vec![BranchCandidate { branch: task_branch.into(), exact_shas: None }];
    if let Err(e) = git::verify_checkout_root(&dir) {
        return BranchEvidence { candidates, errors: vec![e] };
    }
    let mut errors = Vec::new();
    match git::other_branch_checked(&dir, task_branch, &checkout.base) {
        Ok(Some(branch)) => candidates.push(BranchCandidate { branch, exact_shas: None }),
        Ok(None) => {}
        Err(e) => errors.push(e),
    }
    let excluded: Vec<&str> =
        scope.excluded.iter().map(String::as_str).chain(candidates.iter().map(|c| c.branch.as_str())).collect();
    let (departed, error) =
        git::departed_topics(&dir, scope.since, &checkout.base, scope.configured_default.as_deref(), &excluded);
    errors.extend(error);
    candidates.extend(departed.into_iter().map(|e| BranchCandidate { branch: e.branch, exact_shas: Some(e.shas) }));
    BranchEvidence { candidates, errors }
}

#[derive(Debug)]
pub(crate) struct PullRead {
    pub prs: Vec<PullRequest>,
    pub errors: Vec<Error>,
}

impl PullRead {
    pub(crate) fn error_message(&self) -> Option<String> {
        // The existing reconnect UI compares these exact messages.
        let connection = self
            .errors
            .iter()
            .find(|e| matches!(e, Error::TokenRejected(_)))
            .or_else(|| self.errors.iter().find(|e| matches!(e, Error::TokenNeedsSso(_))));
        connection.map(ToString::to_string).or_else(|| {
            (!self.errors.is_empty())
                .then(|| self.errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))
        })
    }
}

/// Keep good branch results alongside local and remote read errors. One
/// configured repository is being queried, so its PR number is its identity.
pub(crate) async fn read_pulls(client: &GitHub, owner: &str, name: &str, evidence: BranchEvidence) -> PullRead {
    let BranchEvidence { candidates, mut errors } = evidence;
    let mut all = Vec::new();
    // Callers already parallelize tasks. Do not multiply that concurrency
    // by the current and historical branch count for each checkout.
    for c in candidates {
        let BranchPulls { prs, error } = client.paged_pulls_for_branch(owner, name, &c.branch).await;
        errors.extend(error);
        all.extend(prs.into_iter().filter(|p| {
            c.exact_shas
                .as_ref()
                .is_none_or(|shas| p.head == c.branch && shas.iter().any(|sha| p.head_sha.eq_ignore_ascii_case(sha)))
        }));
    }
    PullRead { prs: newest_first(all), errors }
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
    use crate::commands::off_runtime;
    use std::path::Path;

    fn pr(number: u64, head: &str) -> PullRequest {
        PullRequest {
            id: None,
            number,
            title: String::new(),
            state: "open".into(),
            draft: false,
            author: String::new(),
            head: head.into(),
            head_repo: None,
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

    fn endpoints(api: &str, web: &str) -> GithubConfig {
        GithubConfig { api_url: api.into(), web_url: web.into(), ..Default::default() }
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
            pr_pin: None,
            pr_pin_revision: 0,
        }
    }

    #[test]
    fn a_folder_an_agent_moved_to_its_own_branch_is_asked_about_that_branch_too() {
        let dir = repo();
        git(&dir, &["checkout", "-q", "-b", "fix/DT-22699-flight-offer-refused"]);
        assert_eq!(
            direct_branches(&checkout(&dir, "main"), "DT-22303"),
            vec!["DT-22303".to_string(), "fix/DT-22699-flight-offer-refused".to_string()],
        );
    }

    #[test]
    fn a_folder_on_the_task_branch_is_asked_about_it_once() {
        let dir = repo();
        git(&dir, &["checkout", "-q", "-b", "DT-22303"]);
        assert_eq!(direct_branches(&checkout(&dir, "main"), "DT-22303"), vec!["DT-22303".to_string()]);
    }

    #[test]
    fn a_folder_on_its_base_or_the_default_branch_or_detached_adds_nothing() {
        let dir = repo();
        git(&dir, &["branch", "development"]);
        git(&dir, &["checkout", "-q", "development"]);
        assert_eq!(direct_branches(&checkout(&dir, "development"), "DT-1"), vec!["DT-1".to_string()]);
        git(&dir, &["checkout", "-q", "main"]);
        assert_eq!(direct_branches(&checkout(&dir, "development"), "DT-1"), vec!["DT-1".to_string()]);
        git(&dir, &["checkout", "-q", "--detach"]);
        assert_eq!(direct_branches(&checkout(&dir, "main"), "DT-1"), vec!["DT-1".to_string()]);
    }

    fn direct_branches(checkout: &Checkout, branch: &str) -> Vec<String> {
        let scope = HistoryScope { since: 0, configured_default: None, excluded: Vec::new() };
        branch_evidence(checkout, branch, &scope)
            .candidates
            .into_iter()
            .filter(|c| c.exact_shas.is_none())
            .map(|c| c.branch)
            .collect()
    }

    #[test]
    fn pull_requests_from_both_branches_are_listed_newest_first_and_once() {
        let prs = newest_first(vec![pr(731, "DT-22303"), pr(763, "fix/x"), pr(731, "DT-22303"), pr(12, "DT-22303")]);
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), vec![763, 731, 12]);
    }

    fn task(id: &str, branch: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "root": "", "branch": branch, "created_at": "2020-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn rev(dir: &Path, at: &str) -> String {
        git::run_for_tests(dir, &["rev-parse", at]).unwrap().trim().to_string()
    }

    fn linked() -> (PathBuf, PathBuf, PathBuf, Checkout) {
        let clone = repo();
        let bare = clone.join("bare.git");
        git(&clone, &["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
        git(&bare, &["config", "core.logAllRefUpdates", "true"]);
        let worktree = clone.join("task-worktree");
        git(&bare, &["worktree", "add", "-qb", "task", worktree.to_str().unwrap(), "main"]);
        let sibling = clone.join("sibling-worktree");
        git(&bare, &["worktree", "add", "-qb", "sibling", sibling.to_str().unwrap(), "main"]);
        let mut c = checkout(&worktree, "main");
        c.base_commit = Some(rev(&worktree, "HEAD"));
        (clone, worktree, sibling, c)
    }

    fn commit(dir: &Path) -> String {
        git(dir, &["-c", "user.email=a@b", "-c", "user.name=a", "commit", "-q", "--allow-empty", "-m", "work"]);
        rev(dir, "HEAD")
    }

    fn topic(dir: &Path, name: &str) -> String {
        git(dir, &["checkout", "-qb", name]);
        let tip = commit(dir);
        git(dir, &["checkout", "-q", "task"]);
        tip
    }

    fn topics(config: &AppConfig, task: &Task, c: &Checkout) -> (Vec<(String, Option<String>)>, bool) {
        let found = branch_evidence(c, &task.branch, &HistoryScope::for_checkout(config, task, c));
        (
            found
                .candidates
                .into_iter()
                .flat_map(|c| {
                    c.exact_shas.map_or_else(
                        || vec![(c.branch.clone(), None)],
                        |shas| shas.into_iter().map(|sha| (c.branch.clone(), Some(sha))).collect(),
                    )
                })
                .collect(),
            !found.errors.is_empty(),
        )
    }

    #[test]
    fn history_proves_the_departure_tip_only_in_that_linked_worktree() {
        let (clone, wt, sibling, c) = linked();
        let (config, task) = (AppConfig::default(), task("t", "task"));
        let at = topic(&wt, "agent/topic");
        assert_ne!(at, rev(&wt, "HEAD"));
        git(&wt, &["checkout", "-qb", "current"]);
        let (found, incomplete) = topics(&config, &task, &c);
        assert!(!incomplete);
        assert_eq!(found, vec![("task".into(), None), ("current".into(), None), ("agent/topic".into(), Some(at))]);
        for dir in [&clone, &sibling] {
            let row = topics(&config, &task, &checkout(dir, "main"));
            assert!(!row.0.iter().any(|(b, _)| b == "agent/topic"));
        }
    }

    #[test]
    fn history_excludes_base_defaults_detached_other_tasks_and_unworked_tips() {
        let (clone, wt, _, mut c) = linked();
        let mut config = AppConfig::default();
        config.projects.push(
            serde_json::from_value(serde_json::json!({
                "id": "p", "name": "demo", "path": clone, "default_branch": "configured-default"
            }))
            .unwrap(),
        );
        config.tasks.push(task("other", "another-task"));
        config.checkouts.push(Checkout { task_id: "other".into(), ..c.clone() });
        c.base = "base".into();
        git(&wt, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/remote-default"]);
        for branch in ["base", "configured-default", "remote-default", "another-task"] {
            topic(&wt, branch);
        }
        git(&wt, &["checkout", "-qb", "no-work"]);
        git(&wt, &["checkout", "-q", "task"]);
        git(&wt, &["checkout", "--detach", "-q"]);
        commit(&wt);
        git(&wt, &["checkout", "-q", "task"]);
        let keep = topic(&wt, "real-topic");

        assert_eq!(
            topics(&config, &task("t", "task"), &c),
            (vec![("task".into(), None), ("real-topic".into(), Some(keep))], false)
        );
    }

    #[test]
    fn history_uses_the_configured_default_without_guessing_a_second_default() {
        let (clone, wt, _, mut c) = linked();
        let mut config = AppConfig::default();
        config.projects.push(
            serde_json::from_value(serde_json::json!({
                "id": "p", "name": "demo", "path": clone, "default_branch": "develop"
            }))
            .unwrap(),
        );
        c.base = "develop".into();
        topic(&wt, "develop");
        git(&wt, &["checkout", "-q", "main"]);
        let work = commit(&wt);
        git(&wt, &["checkout", "-q", "task"]);
        assert_eq!(
            topics(&config, &task("t", "task"), &c),
            (vec![("task".into(), None), ("main".into(), Some(work))], false)
        );
    }

    #[test]
    fn a_missing_adjacent_record_does_not_change_the_departure_tip() {
        let (_, wt, _, c) = linked();
        let tip = topic(&wt, "topic");
        git(&wt, &["reflog", "delete", "HEAD@{1}"]);
        assert_eq!(
            topics(&AppConfig::default(), &task("t", "task"), &c),
            (vec![("task".into(), None), ("topic".into(), Some(tip))], true)
        );
    }

    #[test]
    fn round_one_an_advanced_checkout_point_does_not_erase_a_historical_tip() {
        let (_, wt, _, mut c) = linked();
        let tip = topic(&wt, "topic");
        c.base_commit = Some(tip.clone());
        assert_eq!(
            topics(&AppConfig::default(), &task("t", "task"), &c),
            (vec![("task".into(), None), ("topic".into(), Some(tip))], false)
        );
    }

    #[test]
    fn round_one_an_unknown_start_point_does_not_turn_into_a_complete_read() {
        let (_, wt, _, c) = linked();
        topic(&wt, "topic");
        let path =
            git::run_for_tests(&wt, &["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"]).unwrap();
        let log = std::fs::read_to_string(path.trim()).unwrap();
        git(&wt, &["reflog", "delete", &format!("HEAD@{{{}}}", log.lines().count() - 1)]);
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));
    }

    #[test]
    fn round_two_busy_checkouts_keep_their_start_point_outside_the_departure_window() {
        let (_, wt, _, c) = linked();
        for _ in 0..101 {
            git(&wt, &["reset", "--soft", "HEAD"]);
        }
        let tip = topic(&wt, "recent");
        assert_eq!(
            topics(&AppConfig::default(), &task("t", "task"), &c),
            (vec![("task".into(), None), ("recent".into(), Some(tip))], false)
        );
    }

    #[test]
    fn round_two_a_rewritten_departure_cannot_reuse_the_arrival_tip() {
        let (_, wt, _, c) = linked();
        let arrival = commit(&wt);
        git(&wt, &["checkout", "-qb", "topic"]);
        let real_departure = commit(&wt);
        assert_ne!(arrival, real_departure);
        git(&wt, &["checkout", "-q", "task"]);
        git(&wt, &["reflog", "delete", "--rewrite", "HEAD@{1}"]);
        let path =
            git::run_for_tests(&wt, &["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"]).unwrap();
        let log = std::fs::read_to_string(path.trim()).unwrap();
        let mut lines = log.lines().rev();
        assert!(lines.next().unwrap().starts_with(&arrival));
        assert!(lines.next().unwrap().ends_with("checkout: moving from task to topic"));
        assert_eq!(rev(&wt, "refs/heads/topic"), real_departure);
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));

        // Even a named ref now at A does not prove A was the departure
        // tip if the ref moved again outside this worktree in the same second.
        let stamp: i64 = log
            .lines()
            .last()
            .unwrap()
            .split_once('\t')
            .unwrap()
            .0
            .rsplit_once("> ")
            .unwrap()
            .1
            .split_once(' ')
            .unwrap()
            .0
            .parse()
            .unwrap();
        let rewind = format!("alias.rewind-ref=!GIT_COMMITTER_DATE='@{stamp} +0000' git update-ref");
        git(&wt, &["-c", &rewind, "rewind-ref", "refs/heads/topic", &arrival]);
        assert_eq!(rev(&wt, "refs/heads/topic"), arrival);
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));

        // Cleanup must not make the surviving arrival a proof of the old tip.
        git(&wt, &["branch", "-d", "topic"]);
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));
    }

    #[test]
    fn round_two_renaming_an_attached_topic_keeps_its_exact_departure_tip() {
        let (_, wt, _, c) = linked();
        git(&wt, &["checkout", "-qb", "old-topic"]);
        let tip = commit(&wt);
        git(&wt, &["branch", "-m", "renamed-topic"]);
        git(&wt, &["checkout", "-q", "task"]);
        assert_eq!(
            topics(&AppConfig::default(), &task("t", "task"), &c),
            (vec![("task".into(), None), ("renamed-topic".into(), Some(tip))], false)
        );
    }

    #[test]
    fn round_one_a_lost_git_link_cannot_borrow_the_enclosing_repository() {
        let (clone, wt, _, c) = linked();
        git(&clone, &["checkout", "-qb", "parent-topic"]);
        commit(&clone);
        std::fs::remove_file(wt.join(".git")).unwrap();
        assert!(configured_checkout_repo(&wt, &endpoints("https://api.github.com", "https://github.com")).is_err());
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));
    }

    #[test]
    fn round_one_rewritten_pruning_cannot_give_a_topic_another_branchs_tip() {
        let (_, wt, _, c) = linked();
        git(&wt, &["checkout", "-qb", "decoy"]);
        let wrong = commit(&wt);
        git(&wt, &["checkout", "-qb", "topic"]);
        commit(&wt);
        git(&wt, &["checkout", "-q", "task"]);
        git(&wt, &["reflog", "delete", "--rewrite", "HEAD@{1}"]);
        git(&wt, &["reflog", "delete", "--rewrite", "HEAD@{1}"]);
        let path =
            git::run_for_tests(&wt, &["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"]).unwrap();
        let log = std::fs::read_to_string(path.trim()).unwrap();
        let departure = log.lines().last().unwrap();
        assert!(departure.starts_with(&wrong));
        assert!(departure.ends_with("checkout: moving from topic to task"));
        assert_eq!(topics(&AppConfig::default(), &task("t", "task"), &c), (vec![("task".into(), None)], true));
    }

    #[test]
    fn round_one_equal_slugs_on_different_forges_do_not_establish_repo_identity() {
        let (_, wt, _, _) = linked();
        let repo = |dir: &Path, web: &str| {
            let api =
                if web == "https://github.com" { "https://api.github.com" } else { "https://forge.example/api/v3" };
            configured_checkout_repo(dir, &endpoints(api, web))
        };
        git(&wt, &["config", "remote.origin.url", "ssh://git@FORGE.example:2222/acme/tool.git"]);
        assert_eq!(repo(&wt, "https://forge.example").unwrap(), ("acme".into(), "tool".into()));
        assert!(repo(&wt, "https://github.com").is_err());
        git(&wt, &["config", "remote.origin.url", "git@github.com:acme/tool.git"]);
        assert_eq!(repo(&wt, "https://github.com").unwrap(), ("acme".into(), "tool".into()));
        git(&wt, &["config", "remote.origin.url", "https://github.com:8443/acme/tool"]);
        assert!(repo(&wt, "https://github.com").is_err());
    }

    #[test]
    fn round_two_a_valid_origin_and_web_url_cannot_bless_an_unrelated_api() {
        let (_, wt, _, _) = linked();
        let repo = |api, web| configured_checkout_repo(&wt, &endpoints(api, web));
        git(&wt, &["config", "remote.origin.url", "git@github.com:acme/tool.git"]);
        assert!(repo("https://api.github.com", "https://github.com").is_ok());
        assert!(repo("http://127.0.0.1:3000/api/v3", "https://github.com").is_err());
        assert!(repo("https://api.github.com:8443", "https://github.com").is_err());
        git(&wt, &["config", "remote.origin.url", "git@forge.example:acme/tool.git"]);
        assert!(repo("https://forge.example/api/v3", "https://forge.example").is_ok());
        assert!(repo("https://forge.example:8443/api/v3", "https://forge.example").is_err());
        assert!(repo("https://forge.example/api/v3", "https://another.example").is_err());
        git(&wt, &["config", "remote.origin.url", "git@acme.ghe.com:acme/tool.git"]);
        assert!(repo("https://api.acme.ghe.com", "https://acme.ghe.com").is_ok());
        assert!(repo("https://api.other.ghe.com", "https://acme.ghe.com").is_err());
    }

    #[test]
    fn round_two_github_ssh_over_443_is_the_only_supported_alias() {
        let (_, wt, _, _) = linked();
        let repo = |web| {
            let api =
                if web == "https://github.com" { "https://api.github.com" } else { "https://forge.example/api/v3" };
            configured_checkout_repo(&wt, &endpoints(api, web))
        };
        git(&wt, &["config", "remote.origin.url", "ssh://git@ssh.github.com:443/acme/tool.git"]);
        assert_eq!(repo("https://github.com").unwrap(), ("acme".into(), "tool".into()));
        assert!(repo("https://forge.example").is_err());
        for url in [
            "ssh://git@ssh.github.com:22/acme/tool.git",
            "git@ssh.github.com:acme/tool.git",
            "https://ssh.github.com/acme/tool.git",
            "ssh://git@ssh.other.example:443/acme/tool.git",
        ] {
            git(&wt, &["config", "remote.origin.url", url]);
            assert!(repo("https://github.com").is_err(), "{url}");
        }
    }

    #[test]
    fn history_only_uses_recent_distinct_topics_in_the_tasks_lifetime() {
        let (_, wt, _, c) = linked();
        let t = task("t", "task");
        let config = AppConfig::default();
        topic(&wt, "old-1");
        topic(&wt, "old-2");
        let third = topic(&wt, "third");
        let second = topic(&wt, "second");
        let earlier_first = topic(&wt, "first");
        git(&wt, &["checkout", "-q", "first"]);
        let first = commit(&wt);
        git(&wt, &["checkout", "-q", "task"]);
        assert_eq!(
            topics(&config, &t, &c),
            (
                vec![
                    ("task".into(), None),
                    ("first".into(), Some(first)),
                    ("first".into(), Some(earlier_first)),
                    ("second".into(), Some(second)),
                    ("third".into(), Some(third))
                ],
                false
            )
        );
        let future = Task { created_at: chrono::Utc::now() + chrono::Duration::days(1), ..t.clone() };
        assert_eq!(topics(&config, &future, &c), (vec![("task".into(), None)], false));
        let head = rev(&wt, "HEAD");
        for _ in 0..100 {
            git(&wt, &["reset", "--soft", &head]);
        }
        assert_eq!(topics(&config, &t, &c), (vec![("task".into(), None)], false));
    }

    #[test]
    fn malformed_and_missing_history_do_not_erase_usable_candidates_or_look_complete() {
        use std::io::Write;

        let (_, wt, _, c) = linked();
        let (config, t) = (AppConfig::default(), task("t", "task"));
        let valid = topic(&wt, "topic");
        let reflog =
            git::run_for_tests(&wt, &["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"]).unwrap();
        let reflog = Path::new(reflog.trim());
        std::fs::OpenOptions::new().append(true).open(reflog).unwrap().write_all(b"malformed record\n").unwrap();
        assert_eq!(topics(&config, &t, &c), (vec![("task".into(), None), ("topic".into(), Some(valid))], true));
        std::fs::remove_file(reflog).unwrap();
        assert_eq!(topics(&config, &t, &c), (vec![("task".into(), None)], true));
    }

    #[tokio::test]
    async fn round_three_deleting_the_local_topic_keeps_its_exact_remote_pr() {
        use axum::{extract::Query, routing::get, Json, Router};
        use std::collections::HashMap;

        let (_, wt, _, c) = linked();
        let start = rev(&wt, "HEAD");
        let tip = topic(&wt, "cleaned/topic");
        git(&wt, &["merge", "--ff-only", "cleaned/topic"]);
        git(&wt, &["branch", "-d", "cleaned/topic"]);
        assert!(!git::branch_exists(&wt, "cleaned/topic"));
        let path = git::run_for_tests(
            &wt,
            &["rev-parse", "--path-format=absolute", "--git-path", "logs/refs/heads/cleaned/topic"],
        )
        .unwrap();
        assert!(!Path::new(path.trim()).exists());

        let item = |number, branch: &str, sha: &str| {
            serde_json::json!({
                "number": number, "title": "Fixture PR", "state": "closed", "draft": false,
                "head": {"ref": branch, "sha": sha}, "base": {"ref": "main"},
                "html_url": format!("https://git.example/acme/tool/pull/{number}"), "merged_at": "2026-01-01T00:00:00Z",
            })
        };
        let app = Router::new().route(
            "/api/v3/repos/acme/tool/pulls",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let (tip, start) = (tip.clone(), start.clone());
                async move {
                    Json(match q["head"].as_str() {
                        "acme:task" => vec![item(2, "task", &tip)],
                        "acme:cleaned/topic" => vec![item(9, "cleaned/topic", &tip), item(10, "cleaned/topic", &start)],
                        other => panic!("unexpected head {other}"),
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web = format!("http://{}", listener.local_addr().unwrap());
        git(&wt, &["config", "remote.origin.url", &format!("{web}/acme/tool.git")]);
        let cfg = endpoints(&format!("{web}/api/v3"), &web);
        let client = GitHub::new(&cfg, "local-fixture-token").unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let ((owner, repo), evidence) = off_runtime(move || {
            let t = task("t", "task");
            let repo = configured_checkout_repo(Path::new(&c.path), &cfg).unwrap();
            let evidence = branch_evidence(&c, &t.branch, &HistoryScope::for_checkout(&AppConfig::default(), &t, &c));
            (repo, evidence)
        })
        .await
        .unwrap();
        let read = read_pulls(&client, &owner, &repo, evidence).await;
        assert_eq!(read.prs.iter().map(|p| p.number).collect::<Vec<_>>(), [9, 2]);
        assert!(read.errors.is_empty(), "{:?}", read.errors);
    }

    #[tokio::test]
    async fn history_matches_exact_head_and_keeps_deduplicated_prs_on_branch_failure() {
        use axum::{extract::Query, response::IntoResponse, routing::get, Json, Router};
        use std::collections::HashMap;

        let (_, wt, _, c) = linked();
        let task_tip = rev(&wt, "HEAD");
        let old = topic(&wt, "topic");
        git(&wt, &["checkout", "-q", "topic"]);
        let newer = commit(&wt);
        git(&wt, &["checkout", "-q", "task"]);
        assert_ne!(old, newer);
        git(&wt, &["checkout", "-qb", "present"]);
        let topic_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = topic_calls.clone();
        let fixture = move |n, branch: &str, sha: &str| {
            serde_json::json!({
                "number": n, "title": "A change", "state": "closed",
                "draft": false, "user": {"login": "dev"},
                "head": {"ref": branch, "sha": sha}, "base": {"ref": "main"},
                "html_url": format!("https://git.example/acme/tool/pull/{n}"), "merged_at": "2026-01-01T00:00:00Z",
            })
        };
        let app = Router::new().route(
            "/api/v3/repos/acme/tool/pulls",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let (old, task_tip) = (old.clone(), task_tip.clone());
                let calls = calls.clone();
                async move {
                    match q["head"].as_str() {
                        "acme:task" => Json(vec![fixture(2, "task", &task_tip)]).into_response(),
                        "acme:topic" => {
                            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            Json(vec![
                                fixture(9, "topic", &old),
                                fixture(9, "topic", &old),
                                fixture(10, "topic", &task_tip),
                                fixture(11, "unrelated", &old),
                            ])
                            .into_response()
                        }
                        "acme:present" => (reqwest::StatusCode::UNAUTHORIZED, "fixture").into_response(),
                        _ => panic!("unexpected head"),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = crate::config::GithubConfig {
            api_url: format!("http://{}/api/v3", listener.local_addr().unwrap()),
            ..Default::default()
        };
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = GitHub::new(&cfg, "local-fixture-token").unwrap();
        let evidence = off_runtime(move || {
            let t = task("t", "task");
            branch_evidence(&c, &t.branch, &HistoryScope::for_checkout(&AppConfig::default(), &t, &c))
        })
        .await
        .unwrap();
        let read = read_pulls(&client, "acme", "tool", evidence).await;
        assert_eq!(read.prs.iter().map(|p| p.number).collect::<Vec<_>>(), [9, 2]);
        assert_eq!(topic_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(read.prs.iter().all(|p| p.merged));
        assert_eq!(read.error_message().as_deref(), Some("GitHub no longer accepts the saved token"));
    }
}
