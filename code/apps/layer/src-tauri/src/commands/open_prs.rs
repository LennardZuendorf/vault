//! Opening a task's pull requests (PR-1, PR-2): push each repo with work no
//! pull request has landed, open or reuse its PR, and announce the new ones.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tauri::State;

use crate::config::TrackerKind;
use crate::error::Result;
use crate::git;
use crate::integrations::github;

use super::diff::RepoResult;
use super::pr_resolver::{self, action_target, Connection, Publication, Resolved, Scope};
use super::slack::{record_post, slack_for};
use super::ticket_flow::JiraConnection;
use super::AppState;

#[derive(serde::Serialize)]
pub struct OpenPrRow {
    #[serde(flatten)]
    result: RepoResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_target: Option<String>,
}

impl From<RepoResult> for OpenPrRow {
    fn from(result: RepoResult) -> Self {
        Self { result, image_target: None }
    }
}

type Opened = (github::Sibling, bool);

/// Push and open a PR in every repository that has changes, then post the whole
/// set back to the Jira ticket and Slack, and list in each PR the others.
/// `skip` names checkouts the user left out: not pushed, not opened.
#[tauri::command]
pub async fn github_open_prs(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    title: String,
    body: String,
    draft: bool,
    skip: Vec<String>,
) -> Result<Vec<OpenPrRow>> {
    let mut scope = Scope::capture(&state, &task_id, Instant::now())?;
    let task = scope.task.clone();
    super::pr_task::not_a_review(&task, "pushed or opened as a pull request")?;
    let jira = JiraConnection::load(&state).ok();
    let connection = Arc::new(Connection::load(&state).await?);
    let linked_tracker = state.config.with(|c| {
        let key = task.issue_key.as_deref()?;
        let url = crate::tickets::url_of(c, key)?;
        (task.issue_url.as_deref() == Some(url.as_str())).then(|| c.tracker_in_use()).flatten()
    });
    let body = fixes(linked_tracker, task.issue_key.as_deref(), body);

    let mut results = Vec::new();
    let mut opened: Vec<github::Sibling> = Vec::new();
    // Every open PR of the task, new or not: one opened earlier does not know
    // about a repo added since.
    let mut set: Vec<github::Sibling> = Vec::new();

    for checkout_id in scope.checkout_ids().map(str::to_owned).collect::<Vec<_>>() {
        if skip.contains(&checkout_id) {
            continue;
        }
        let checkout = state.config.checkout(&checkout_id)?;
        let repo = match state.config.project(&checkout.project_id) {
            Ok(project) => project.name,
            Err(e) => {
                results.push(
                    RepoResult { checkout_id: checkout.id, repo: "(unknown)".into(), ok: false, detail: e.to_string() }
                        .into(),
                );
                continue;
            }
        };
        // Steps agents ticked since the spec was approved go to review with
        // the code (SPEC-13).
        if let Err(e) = {
            let (task, id) = (task.clone(), checkout.id.clone());
            super::blocking(app.clone(), move |state| super::spec_work::commit_ticks(state, &task, &id)).await
        } {
            results.push(
                RepoResult {
                    checkout_id: checkout.id,
                    repo,
                    ok: false,
                    detail: format!("the spec's ticked steps could not be committed: {e}"),
                }
                .into(),
            );
            continue;
        }

        let outcome: Result<(String, Option<Opened>, Option<String>)> = async {
            let mut resolved = scope.checkout(&connection, &checkout_id, false).await?;
            scope.advance_rows(&state, &connection, std::slice::from_mut(&mut resolved)).await?;
            let publication = resolved.publication(&task.branch)?;
            if !scope.is_current(&state, &connection) {
                return Err(pr_resolver::stale());
            }
            let (owner, name) = resolved.slug()?;
            let sibling = |pr: &github::PullRequest, editable| -> Result<github::Sibling> {
                let url = connection.validated_pr_link(&format!("{owner}/{name}"), pr)?;
                Ok(github::Sibling {
                    repo: repo.clone(),
                    owner: owner.into(),
                    name: name.into(),
                    url,
                    number: pr.number,
                    id: pr.id,
                    editable,
                })
            };
            let existing = match publication {
                Publication::Alternate(pr) => {
                    let link = sibling(pr, false)?;
                    return Ok((
                        format!(
                            "#{} already open: {} — no PR opened or branch pushed; sibling edit skipped",
                            pr.number, link.url
                        ),
                        Some((link, false)),
                        None,
                    ));
                }
                Publication::Reuse(pr) => Some(pr.clone()),
                Publication::Create => None,
            };
            let dir = PathBuf::from(&checkout.path);
            let (s, c, checked) = (scope.clone(), connection.clone(), resolved.clone());
            let pushed = super::blocking(app.clone(), move |state| {
                s.require_current(state, &c, std::slice::from_ref(&checked))?;
                let branch = &s.task.branch;
                let checkout = &checked.checkout;
                let point = checked.publish_point(branch);
                if checked.task_ref_landed || !git::task_branch_has_commits(&dir, branch, &checkout.base, point)? {
                    return Ok(false);
                }
                git::push(&dir, branch, checkout.push_lease.as_deref())?;
                super::diff::pushed(state, &checkout.id);
                Ok(true)
            })
            .await?;
            if !pushed && existing.is_none() {
                return Ok((resolved.nothing_to_publish()?, None, None));
            }
            if !scope.is_current(&state, &connection) {
                return Err(pr_resolver::stale());
            }
            let (pr, new) = match existing {
                Some(existing) => (existing, false),
                None => {
                    // A push can wait on the remote. Resolve again before
                    // a POST: an agent may have opened a PR in the meantime.
                    let after_push = scope.checkout(&connection, &checkout_id, false).await?;
                    let after_slug = after_push.slug()?;
                    if after_slug != (owner, name) || !scope.is_current(&state, &connection) {
                        return Err(pr_resolver::stale());
                    }
                    match after_push.publication(&task.branch)? {
                        Publication::Reuse(pr) => (pr.clone(), false),
                        Publication::Alternate(pr) => {
                            let link = sibling(pr, false)?;
                            return Ok((
                                format!(
                                    "#{} already open: {} — pushed the task branch, no PR opened; sibling edit skipped",
                                    pr.number, link.url
                                ),
                                Some((link, false)),
                                None,
                            ));
                        }
                        Publication::Create => {
                            let (s, c) = (scope.clone(), connection.clone());
                            super::blocking(app.clone(), move |state| {
                                require_create_evidence(state, &s, &c, &after_push)
                            })
                            .await?;
                            (
                                connection
                                    .client()?
                                    .create_pull(owner, name, &title, &body, &task.branch, &checkout.base, draft)
                                    .await?,
                                true,
                            )
                        }
                    }
                }
            };
            let detail = if new {
                format!("#{}", pr.number)
            } else if pushed {
                format!("#{} already open, pushed", pr.number)
            } else {
                format!("#{} already open", pr.number)
            };
            let pinned = resolved.row.pin.as_ref().is_some_and(|pin| pin.number == pr.number);
            let link = sibling(&pr, !pinned)?;
            let target = action_target(&connection, &resolved.checkout, (owner, name), &pr);
            if pinned {
                return Ok((format!("{detail}; pinned PR sibling edit skipped"), Some((link, new)), target));
            }
            Ok((detail, Some((link, new)), target))
        }
        .await;

        match outcome {
            Ok((detail, pr, image_target)) => {
                results.push(OpenPrRow {
                    result: RepoResult { checkout_id: checkout.id, repo, ok: true, detail },
                    image_target,
                });
                if let Some((pr, new)) = pr {
                    set.push(pr.clone());
                    if new {
                        opened.push(pr);
                    }
                }
            }
            Err(e) => {
                results.push(RepoResult { checkout_id: checkout.id, repo, ok: false, detail: e.to_string() }.into())
            }
        }
    }

    if !opened.is_empty() && set.len() > 1 {
        for (repo, e) in connection
            .client()?
            .link_siblings(&set, task.issue_key.as_deref(), || scope.is_current(&state, &connection))
            .await
        {
            results.push(
                RepoResult {
                    checkout_id: repo.clone(),
                    repo,
                    ok: false,
                    detail: format!("the PR is open, but listing the others in it failed ({e})"),
                }
                .into(),
            );
        }
    }

    if !opened.is_empty() {
        let lines: Vec<String> = opened.iter().map(|p| format!("{}: {}", p.repo, p.url)).collect();

        // A GitHub issue lists the pull requests that fix it by itself.
        if let Some((jira, key)) = jira.as_ref().and_then(|jira| jira.key(&task).map(|key| (jira, key))) {
            if scope.is_current(&state, &connection) && jira.current(&state) {
                let text = format!(
                    "Pull request{} for this ticket:\n{}",
                    if opened.len() == 1 { "" } else { "s" },
                    lines.join("\n"),
                );
                // Said, not swallowed: this only runs for PRs opened by this
                // call, so a retry finds them already open and never posts —
                // the ticket that is meant to be the hub would just not know.
                if let Err(e) = jira.client.comment(key, &text).await {
                    results.push(RepoResult {
                        checkout_id: key.into(),
                        repo: key.into(),
                        ok: false,
                        detail: format!(
                            "the PRs are open, but the comment linking them failed ({e}); add them to the ticket by hand"
                        ),
                    }.into());
                }
            }
        }

        if let Ok(Some((client, cfg))) = slack_for(&state, "prs") {
            let text = format!("*{title}* — {} PR{} opened", opened.len(), if opened.len() == 1 { "" } else { "s" },);
            let context = opened
                .iter()
                .map(|p| format!("<{}|{} #{}>", p.url, p.repo, p.number))
                .collect::<Vec<_>>()
                .join("  ·  ");
            if let Ok(posted) = client.post(&cfg.channel, &text, Some(&context)).await {
                record_post(&state, posted);
            }
        }
    }

    // What was just opened is the sweep's to follow from now (PR-8).
    let _generation = state.config.tracker_credential_generation(TrackerKind::Github);
    pr_resolver::invalidate(&state, &task_id);
    Ok(results)
}

pub(super) fn require_create_evidence(
    state: &AppState,
    scope: &Scope,
    connection: &Connection,
    row: &Resolved,
) -> Result<()> {
    let rows = std::slice::from_ref(row);
    scope.require_current(state, connection, rows)?;
    scope.require_branch_evidence(rows)?;
    scope.is_current(state, connection).then_some(()).ok_or_else(pr_resolver::stale)
}

/// A pull request for a GitHub issue says it fixes it, which is how GitHub
/// links the two and closes the issue on merge (TKT-18).
fn fixes(tracker: Option<TrackerKind>, key: Option<&str>, body: String) -> String {
    let Some(key) = key.filter(|_| tracker == Some(TrackerKind::Github)) else {
        return body;
    };
    let line = format!("Fixes {key}");
    if body.to_lowercase().contains(&line.to_lowercase()) {
        return body;
    }
    match body.trim_end() {
        "" => line,
        text => format!("{text}\n\n{line}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::repos::tests::{commit, git};
    use super::*;

    #[test]
    fn a_github_issue_pull_request_says_it_fixes_it_once() {
        let gh = Some(TrackerKind::Github);
        assert_eq!(fixes(gh, Some("acme/web#12"), "Loops no more.\n".into()), "Loops no more.\n\nFixes acme/web#12");
        assert_eq!(fixes(gh, Some("acme/web#12"), "fixes acme/web#12".into()), "fixes acme/web#12");
        assert_eq!(fixes(gh, Some("acme/web#12"), String::new()), "Fixes acme/web#12");
        // Jira moves its tickets itself, and a task without one fixes nothing.
        assert_eq!(fixes(Some(TrackerKind::Jira), Some("ACME-1"), "x".into()), "x");
        assert_eq!(fixes(gh, None, "x".into()), "x");
    }

    #[test]
    fn only_committed_work_counts_as_committed() {
        let dir = std::env::temp_dir().join(format!("vl-open-prs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@villain.local"]);
        git(&dir, &["config", "user.name", "Test"]);
        commit(&dir, "a.txt");
        git(&dir, &["checkout", "-q", "-b", "task"]);
        let point = git(&dir, &["rev-parse", "HEAD"]).trim().to_string();

        std::fs::write(dir.join("draft.txt"), "draft\n").unwrap();
        assert!(!git::task_branch_has_commits(&dir, "task", "main", Some(&point)).unwrap());
        commit(&dir, "infra.tf");
        assert!(git::task_branch_has_commits(&dir, "task", "main", Some(&point)).unwrap());
        git(&dir, &["checkout", "-qb", "topic", "main"]);
        assert!(
            git::task_branch_has_commits(&dir, "task", "main", Some(&point)).unwrap(),
            "Open PRs publishes the task ref even when HEAD has no commits"
        );
        git(&dir, &["branch", "other", "main"]);
        commit(&dir, "topic-only");
        assert!(
            !git::task_branch_has_commits(&dir, "other", "main", Some(&point)).unwrap(),
            "topic commits do not make the task ref publishable"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
