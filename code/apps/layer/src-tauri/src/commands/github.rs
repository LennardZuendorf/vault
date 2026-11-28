//! GitHub connect, PR listing, retarget. Opening PRs is in `open_prs.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config::{Checkout, GithubConfig, PrPin, TrackerKind};
use crate::error::{Error, Result};
use crate::git;
use crate::integrations::github::{self, CodeLine, GitHub, PullRequest};
use crate::secrets;

use super::panes::agent_file_dir;
use super::pr_resolver::{Connection, Scope};
use super::AppState;

// ------------------------------------------------------------------ github

pub(crate) fn github_client(state: &AppState) -> Result<(GitHub, GithubConfig)> {
    let cfg = state.config.with(|c| c.github.clone()).ok_or(Error::NotConfigured("GitHub"))?;
    let token = secrets::get(secrets::GITHUB)?.ok_or(Error::NotConfigured("GitHub"))?;
    Ok((GitHub::new(&cfg, &token)?, cfg))
}

#[tauri::command]
pub async fn github_connect<R: tauri::Runtime>(
    app: AppHandle<R>,
    api_url: String,
    web_url: String,
    token: String,
    review_team: Option<String>,
) -> Result<String> {
    let cfg = GithubConfig {
        api_url: api_url.trim_end_matches('/').to_string(),
        web_url: web_url.trim_end_matches('/').to_string(),
        review_team: review_team.as_deref().and_then(github::normalize_review_team),
    };
    let typed = !token.trim().is_empty();
    let (cfg, token, origin, version) = super::blocking(app.clone(), move |state| {
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        let origin = super::pr_pins::api_origin(&cfg)?;
        let token = if typed { token.trim().to_owned() } else { secrets::github_credential()?.for_origin(&origin)? };
        Ok((cfg, token, origin, *generation))
    })
    .await?;
    let login = match GitHub::new(&cfg, &token)?.connection_login().await {
        // A token just typed in is not the saved one yet.
        Err(Error::TokenRejected(_)) if typed => Err(Error::Other("GitHub did not accept this token".into())),
        other => other,
    }?;
    super::blocking(app, move |state| {
        state.config.update_github_credentials(Some(version), Some((&token, &origin)), |c| c.github = Some(cfg))
    })
    .await?;
    Ok(login)
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckoutPr {
    pub checkout_id: String,
    pub repo: String,
    pub pin: Option<PrPin>,
    pub pin_revision: String,
    /// The numbered lookup verified the saved pin, not just an automatic result with its number.
    pub pin_read: bool,
    pub pr: Option<crate::integrations::github::PullRequest>,
    pub checks: Vec<crate::integrations::github::CheckRun>,
    /// Every review submitted on `pr`, so the panel can name who said what.
    pub reviews: Vec<crate::integrations::github::Review>,
    /// Other associated PRs, in the same open-first priority as the primary.
    pub past: Vec<crate::integrations::github::PullRequest>,
    /// The decision those reviews add up to: "approved", "changes_requested",
    /// "commented" or "none".
    pub verdict: String,
    /// Where this repository's next PR will be opened against. An open PR
    /// keeps whatever base it was created with until it is retargeted, so
    /// this and `pr.base` can differ, and the panel says so when they do.
    pub base: String,
    /// Files changed on the branch since its branch point, which a merged PR
    /// moves up to what it landed (UPD-6).
    pub changed: usize,
    /// Threads and comments on the open `pr` waiting on its author (PR-6).
    pub to_reply: u32,
    pub error: Option<String>,
    /// The task branch and checked-out revision contain no uncovered commits.
    pub branch_covered: bool,
    /// Finish cannot preserve unlanded commits on a detached HEAD.
    pub detached_unlanded: bool,
    /// A merge that belongs to this work or covers its actual commits.
    pub covered_merge: bool,
    /// Opaque identity of the open PR shown for actions on its card.
    pub action_target: Option<String>,
}

impl CheckoutPr {
    pub(super) fn unread(checkout: &Checkout, name: &str) -> Self {
        Self {
            checkout_id: checkout.id.clone(),
            repo: name.into(),
            base: checkout.base.clone(),
            pin: checkout.pr_pin.as_ref().map(|p| PrPin { url: String::new(), ..p.pin.clone() }),
            pin_revision: checkout.pr_pin_revision.to_string(),
            pin_read: false,
            pr: None,
            past: Vec::new(),
            checks: Vec::new(),
            reviews: Vec::new(),
            verdict: "none".into(),
            changed: 0,
            to_reply: 0,
            error: None,
            branch_covered: false,
            detached_unlanded: false,
            covered_merge: false,
            action_target: None,
        }
    }

    pub(super) fn pulls(&self) -> impl Iterator<Item = &PullRequest> {
        self.pr.iter().chain(&self.past)
    }

    pub(super) fn open(&self) -> Option<&PullRequest> {
        self.pulls().find(|p| p.state == "open")
    }
}

pub(super) fn task_merged(rows: &[CheckoutPr]) -> bool {
    rows.iter().any(|r| r.covered_merge)
        && rows.iter().all(|r| r.error.is_none() && r.branch_covered && r.changed == 0 && r.open().is_none())
}

/// Point a checkout's pull requests at a different branch.
///
/// The base is chosen when the worktree is made, from the repository's own
/// default branch, and there was no way to say otherwise — work meant for a
/// long-lived integration branch had to be retargeted by hand on GitHub every
/// time. This decides where the *next* PR is opened; one already open keeps
/// its base until `github_retarget_pr` moves it.
#[tauri::command]
pub async fn set_checkout_base(state: State<'_, AppState>, checkout_id: String, base: String) -> Result<()> {
    set_base(&state, &checkout_id, &base)
}

/// [`set_checkout_base`], for agents too (`set_base`).
pub(crate) fn set_base(state: &AppState, checkout_id: &str, base: &str) -> Result<()> {
    let base = base.trim().to_string();
    if base.is_empty() {
        return Err(Error::Other("a pull request needs a base branch".into()));
    }
    // It reaches git where options are still read, on a timer: a base of
    // `--output=<file>` would have written that file every PR sweep.
    if base.starts_with('-') {
        return Err(Error::Other(format!("{base:?} is not a branch name")));
    }
    state.config.update(|c| {
        let Some(found) = c.checkouts.iter_mut().find(|c| c.id == checkout_id) else {
            return Err(Error::NotFound(format!("checkout {checkout_id}")));
        };
        found.base = base.clone();
        // The recorded branch point belongs to the base it was taken from, and
        // `baseline` uses it as-is — keeping it would measure this branch
        // against where it left a branch it is no longer going to. Dropped, so
        // the diff falls back to the merge base with whatever the base is now.
        found.base_commit = None;
        Ok(())
    })?
}

/// What each of a task's branches is measured against, one row per repository.
#[derive(Debug, Serialize)]
pub struct RepoBranchFacts {
    pub checkout_id: String,
    pub repo: String,
    pub base: String,
    #[serde(flatten)]
    pub facts: git::BranchFacts,
}

/// Off the command thread: `branch_facts` is half a dozen git calls per
/// repository, and the Diff view asks for all of them again every time its
/// file list is rebuilt.
#[tauri::command]
pub async fn task_branch_facts(app: AppHandle, task_id: String) -> Result<Vec<RepoBranchFacts>> {
    super::blocking(app, move |state| task_branch_facts_inner(state, task_id)).await
}

fn task_branch_facts_inner(state: &AppState, task_id: String) -> Result<Vec<RepoBranchFacts>> {
    let task = state.config.task(&task_id)?;
    Ok(state
        .config
        .checkouts_of(&task_id)
        .into_iter()
        .map(|c| RepoBranchFacts {
            repo: state.config.project(&c.project_id).map(|p| p.name).unwrap_or_else(|_| "(unknown)".into()),
            facts: git::branch_facts(&PathBuf::from(&c.path), &task.branch, &c.base, c.base_commit.as_deref()),
            base: c.base,
            checkout_id: c.id,
        })
        .collect())
}

/// What this repository could open a pull request against.
#[tauri::command]
pub async fn checkout_branches(app: AppHandle, checkout_id: String) -> Result<Vec<String>> {
    super::blocking(app, move |state| {
        let checkout = state.config.checkout(&checkout_id)?;
        git::remote_branches(&PathBuf::from(&checkout.path))
    })
    .await
}

/// Move the open pull request for a checkout onto its current base.
#[tauri::command]
pub async fn github_retarget_pr(
    app: AppHandle,
    state: State<'_, AppState>,
    checkout_id: String,
    target: String,
) -> Result<String> {
    let checkout = state.config.checkout(&checkout_id)?;
    let scope = Scope::capture(&state, &checkout.task_id, Instant::now())?;
    let connection = Connection::load(&state).await?;
    let result = scope.checkout(&connection, &checkout_id, false).await?;
    let (owner, repo) = result.slug()?;
    let pr = result.require_target(&target)?;
    connection.client()?.verify_edit_target(owner, repo, pr).await?;
    let (number, owner, repo, base) = (pr.number, owner.to_owned(), repo.to_owned(), result.checkout.base.clone());
    let connection = super::blocking(app, move |state| {
        scope.require_current(state, &connection, &[result])?;
        Ok(connection)
    })
    .await?;
    connection.client()?.set_pull_base(&owner, &repo, number, &base).await?;
    Ok(format!("#{number} now targets {base}"))
}

/// Every task's rows in one sweep, for the watch that polls in the background.
#[derive(Debug, Serialize)]
pub struct TaskPrs {
    pub task_id: String,
    pub rows: Vec<CheckoutPr>,
    /// What was done about its ticket in this sweep (TKT-8).
    pub ticket: Option<super::TicketMove>,
}

/// PR state for every repository in the task, one row each.
#[tauri::command]
pub async fn github_task_prs(state: State<'_, AppState>, task_id: String) -> Result<Vec<CheckoutPr>> {
    github_task_prs_from(&state, &task_id, Connection::load_with(&state, secrets::github_credential)).await
}

pub(super) async fn github_task_prs_from(
    state: &AppState,
    task_id: &str,
    connection: impl std::future::Future<Output = Result<Connection>>,
) -> Result<Vec<CheckoutPr>> {
    let mut scope = Scope::capture(state, task_id, Instant::now())?;
    let connection = match connection.await {
        Ok(connection) => connection,
        Err(Error::NotConfigured("GitHub")) => {
            return scope.accept_unread(state, None, GITHUB_DISCONNECTED.into());
        }
        Err(error) => return Err(error),
    };
    if connection.credentials_unreadable() {
        return scope.accept_unread(state, Some(&connection), Error::GithubCredentialUnreadable.to_string());
    }
    let mut read = scope.all(&connection, true).await?;
    scope.accept_rows(state, &connection, &mut read, false).await
}

const GITHUB_DISCONNECTED: &str = "GitHub is not connected. Connect it to read pull requests.";

/// The same for every task, over one client.
///
/// A task whose rows cannot be read is left out rather than failing the sweep:
/// the watch runs unattended, and one unreadable repo must not blind the rest.
///
/// A task whose pull requests were all closed or merged is left out until
/// it is due again (`SETTLED_SWEEP`); the window keeps its last rows.
#[tauri::command]
pub async fn github_all_prs(state: State<'_, AppState>) -> Result<Vec<TaskPrs>> {
    github_all_prs_from(&state, Connection::load_with(&state, secrets::github_credential)).await
}

pub(super) async fn github_all_prs_from(
    state: &AppState,
    connection: impl std::future::Future<Output = Result<Connection>>,
) -> Result<Vec<TaskPrs>> {
    let began = Instant::now();
    let loaded = connection.await;
    let (connection, reason) = match &loaded {
        Err(Error::NotConfigured("GitHub")) => (None, Some(GITHUB_DISCONNECTED.to_owned())),
        Ok(connection) if connection.credentials_unreadable() => {
            (Some(connection), Some(Error::GithubCredentialUnreadable.to_string()))
        }
        Ok(connection) => (Some(connection), None),
        Err(_) => (None, None),
    };
    if let Some(reason) = reason {
        let out = sweep_task_ids(state)
            .into_iter()
            .filter_map(|task_id| {
                let scope = Scope::capture(state, &task_id, began).ok()?;
                let rows = scope.accept_unread(state, connection, reason.clone()).ok()?;
                Some(TaskPrs { task_id, rows, ticket: None })
            })
            .collect();
        super::henchman_watch::swept();
        return Ok(out);
    }
    let connection = &loaded?;
    let ids = sweep_ids(state);

    // A few tasks at a time rather than one after another: with every task's
    // repos in series a sweep of a dozen tasks took longer than the interval
    // between sweeps. Not all at once either — GitHub throttles a client that
    // opens dozens of requests together.
    use futures_util::stream::{self, StreamExt};
    // Tickets follow only with Jira connected; the sweep runs without it.
    let jira = super::ticket_flow::JiraConnection::load(state).ok();
    let jira = &jira;
    let out = stream::iter(ids)
        .map(|task_id| async move {
            let mut scope = Scope::capture(state, &task_id, began).ok()?;
            let mut read = scope.all(connection, true).await.ok()?;
            let rows = scope.accept_rows(state, connection, &mut read, true).await.ok()?;
            let ticket = match jira {
                Some(j) => super::ticket_flow::follow(state, j, &scope, connection, &rows, &read).await,
                None => None,
            };
            Some((scope, TaskPrs { task_id, rows, ticket }))
        })
        .buffered(SWEEP_TASKS)
        .filter_map(|t| async move { t })
        .collect::<Vec<_>>()
        .await;
    // The Henchman's own sweep waits while the window's runs (HEN-12).
    super::henchman_watch::swept();
    Ok(out.into_iter().filter_map(|(scope, row)| scope.is_current(state, connection).then_some(row)).collect())
}

/// The tasks a sweep reads: every one but a review task left from 0.5.0,
/// which has no pull request of its own on its branch and whose ticket is
/// someone else's to move, and a settled one only when it is due.
pub(crate) fn sweep_ids(state: &AppState) -> Vec<String> {
    let ids = sweep_task_ids(state);
    let mut settled = state.prs_settled.lock();
    settled.retain(|id, _| ids.contains(id));
    due(ids, &settled, std::time::Instant::now())
}

fn sweep_task_ids(state: &AppState) -> Vec<String> {
    state.config.with(|c| c.tasks.iter().filter(|t| t.review.is_none()).map(|t| t.id.clone()).collect())
}

/// How many tasks the PR sweep asks GitHub about at once.
pub(crate) const SWEEP_TASKS: usize = 4;

/// How often the sweep asks about a task whose pull requests were all
/// closed or merged. Every task kept was swept every 90 seconds, done ones
/// included: a `git diff`, an `ls-files` walk of the worktree and a GitHub
/// call per repository, forever, and the app got slower the more tasks it
/// kept. A finished PR is reopened rarely, and the PR tab still asks at once.
const SETTLED_SWEEP: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The tasks a sweep asks about: every one, but a settled one only once
/// `SETTLED_SWEEP` has passed since it was last read.
fn due(ids: Vec<String>, settled: &HashMap<String, std::time::Instant>, now: std::time::Instant) -> Vec<String> {
    ids.into_iter().filter(|id| settled.get(id).is_none_or(|at| now.duration_since(*at) >= SETTLED_SWEEP)).collect()
}

/// Whether a task's rows are settled: some pull request, and every repo's
/// newest one closed or merged, or none and nothing on its branch to open
/// one for. A row that could not be read is not settled.
pub(super) fn settled(rows: &[CheckoutPr]) -> bool {
    rows.iter().any(|r| r.pr.is_some())
        && rows.iter().all(|r| {
            r.error.is_none()
                && r.open().is_none()
                && match &r.pr {
                    Some(pr) => pr.state != "open",
                    None => r.changed == 0,
                }
        })
}

/// Pull requests waiting on the signed-in user, and on the configured team.
#[derive(Debug, Serialize)]
pub struct ReviewQueue {
    pub mine: Vec<github::ReviewRequest>,
    /// GitHub had more personal hits than the one page we asked for.
    pub mine_more: bool,
    /// Absent when no review team is configured. Present, with `error` set,
    /// when the team was configured but could not be resolved or searched —
    /// the personal list is still worth showing.
    pub team: Option<TeamReviews>,
    /// The other side: pull requests you opened (REV-4). Only the view asks
    /// for them; the watch that banners new requests has no use for them.
    pub authored: Option<AuthoredPrs>,
}

#[derive(Debug, Serialize)]
pub struct AuthoredPrs {
    pub prs: Vec<github::AuthoredPr>,
    pub more: bool,
    /// Its own error, as the team's: it does not take the review lists down.
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TeamReviews {
    /// `org/slug`.
    pub slug: String,
    pub name: String,
    pub prs: Vec<github::ReviewRequest>,
    pub more: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn github_review_queue(app: AppHandle, state: State<'_, AppState>) -> Result<ReviewQueue> {
    let (queue, authored) = tokio::join!(review_queue(&state), authored_prs(&state));
    let mut queue = queue?;
    crate::news::saw_reviews(&app, &queue);
    queue.authored = Some(authored);
    Ok(queue)
}

/// A failure here is a note on its own section (REV-5), never the queue's.
async fn authored_prs(state: &AppState) -> AuthoredPrs {
    let got = match github_client(state) {
        Ok((client, _)) => client.authored_prs().await,
        Err(e) => Err(e),
    };
    match got {
        Ok((prs, more)) => AuthoredPrs { prs, more, error: None },
        Err(e) => AuthoredPrs { prs: Vec::new(), more: false, error: Some(e.to_string()) },
    }
}

/// The queue itself, for the UI and for the watch that banners what is new.
pub(crate) async fn review_queue(state: &AppState) -> Result<ReviewQueue> {
    let (client, cfg) = github_client(state)?;
    // The two searches need nothing from each other, so they go together;
    // only the de-duplication waits for both.
    let team = async {
        match cfg.review_team.as_deref() {
            Some(raw) => Some(load_team_reviews(&client, &cfg.api_url, raw).await),
            None => None,
        }
    };
    let (mine, team) = tokio::join!(client.requested_prs(github::mine_review_query()), team);
    let (mine, mine_more) = mine?;
    let team = team.map(|mut t| {
        github::drop_already_listed(&mut t.prs, &mine);
        t
    });
    Ok(ReviewQueue { mine, mine_more, team, authored: None })
}

/// The last review team resolved, keyed by the API and the setting it came
/// from.
///
/// A team given without its org is found by walking `/user/teams`, up to ten
/// pages, and that walk was repeated on every poll of the review queue for an
/// answer that changes when the setting does.
static RESOLVED_TEAM: parking_lot::Mutex<Option<(String, String, github::GhTeam)>> = parking_lot::Mutex::new(None);

/// The team column must not take the personal one down with it: a missing
/// `read:org` scope, or a slug that matches nothing, is a problem with that
/// column alone.
async fn load_team_reviews(client: &GitHub, api_url: &str, raw: &str) -> TeamReviews {
    let fallback = raw.trim_start_matches('@').to_string();
    let cached = RESOLVED_TEAM
        .lock()
        .as_ref()
        .filter(|(api, setting, _)| api == api_url && setting == raw)
        .map(|(_, _, team)| team.clone());
    let team = match cached {
        Some(team) => team,
        None => match client.resolve_team(raw).await {
            Ok(team) => {
                *RESOLVED_TEAM.lock() = Some((api_url.to_string(), raw.to_string(), team.clone()));
                team
            }
            Err(e) => {
                return TeamReviews {
                    slug: fallback.clone(),
                    name: fallback,
                    prs: Vec::new(),
                    more: false,
                    error: Some(e.to_string()),
                };
            }
        },
    };
    let slug = format!("{}/{}", team.org, team.slug);
    match client.requested_prs(&github::team_review_query(&team.org, &team.slug)).await {
        Ok((prs, more)) => TeamReviews { slug, name: team.name, prs, more, error: None },
        Err(e) => TeamReviews { slug, name: team.name, prs: Vec::new(), more: false, error: Some(e.to_string()) },
    }
}

pub(super) async fn task_prs(state: &AppState, connection: &Connection, task_id: &str) -> Result<Vec<CheckoutPr>> {
    let mut scope = Scope::capture(state, task_id, Instant::now())?;
    let mut rows = scope.all(connection, true).await?;
    scope.accept_rows(state, connection, &mut rows, true).await
}

/// What reviewers and CI have said on one repository's open pull request.
#[derive(Debug, Serialize)]
pub struct RepoFeedback {
    pub checkout_id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(flatten)]
    pub feedback: github::PrFeedback,
    pub checks: Vec<github::FailedCheck>,
    pub error: Option<String>,
}

/// How many failed jobs' logs are read per repository. A matrix build that
/// fails everywhere fails for one reason, and forty logs say it forty times.
const LOGS_PER_REPO: usize = 6;

/// Everything said on the task's open pull requests, for picking what to
/// hand the agent.
#[tauri::command]
pub async fn github_pr_feedback(state: State<'_, AppState>, task_id: String) -> Result<Vec<RepoFeedback>> {
    let scope = Scope::capture(&state, &task_id, Instant::now())?;
    let connection = Connection::load(&state).await?;
    let resolved = scope.all(&connection, false).await?;
    let rows = resolved.into_iter().map(|resolved| {
        let connection = &connection;
        async move {
            let repo = resolved.row.repo.clone();
            let found: Result<Option<RepoFeedback>> = async {
                let (owner, name) = resolved.slug()?;
                let Some(pr) = resolved.row.open() else {
                    resolved.require_complete()?;
                    return Ok(None);
                };
                let client = connection.client()?;
                // The commit GitHub has, not the local branch: what was checked
                // is what was pushed, and local commits since have no runs yet.
                let (feedback, checks) = tokio::join!(
                    client.pr_feedback(owner, name, pr.number),
                    client.failed_checks(owner, name, &pr.head_sha),
                );
                let mut checks = checks.unwrap_or_default();
                // Counted among the checks that have a log to read: six failing
                // external checks listed first left the Actions job without one.
                let jobs: Vec<(usize, u64)> = checks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| c.job_id.map(|id| (i, id)))
                    .take(LOGS_PER_REPO)
                    .collect();
                let logs = futures_util::future::join_all(
                    jobs.iter().map(|&(_, id)| async move { client.job_log_tail(owner, name, id).await.ok() }),
                )
                .await;
                for (&(i, _), log) in jobs.iter().zip(logs) {
                    checks[i].log = log.filter(|l| !l.is_empty());
                }
                Ok(Some(RepoFeedback {
                    checkout_id: resolved.checkout.id.clone(),
                    repo: repo.clone(),
                    number: pr.number,
                    title: pr.title.clone(),
                    url: pr.url.clone(),
                    feedback: feedback?,
                    checks,
                    error: resolved.row.error.clone(),
                }))
            }
            .await;
            match found {
                Ok(row) => row,
                // Said on the row, so one repository GitHub will not answer
                // for does not hide what the others' reviewers wrote.
                Err(e) => Some(RepoFeedback {
                    checkout_id: resolved.checkout.id,
                    repo,
                    number: 0,
                    title: String::new(),
                    url: String::new(),
                    feedback: Default::default(),
                    checks: Vec::new(),
                    error: Some(e.to_string()),
                }),
            }
        }
    });
    let rows = futures_util::future::join_all(rows).await.into_iter().flatten().collect();
    if !scope.is_current(&state, &connection) {
        return Err(Error::Other(
            "The task's pull request evidence changed during the read. Refresh and try again.".into(),
        ));
    }
    Ok(rows)
}

/// One piece of feedback chosen to hand to the agent, sent back as shown.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedbackItem {
    Thread {
        checkout_id: String,
        path: String,
        line: Option<u64>,
        #[serde(default)]
        start_line: Option<u64>,
        #[serde(default)]
        code: Vec<CodeLine>,
        #[serde(default)]
        outdated: bool,
        #[serde(default)]
        resolved: bool,
        url: String,
        comments: Vec<FeedbackNote>,
    },
    Review {
        checkout_id: String,
        author: String,
        state: Option<String>,
        body: String,
        url: String,
    },
    Comment {
        checkout_id: String,
        author: String,
        body: String,
        url: String,
    },
    Check {
        checkout_id: String,
        name: String,
        conclusion: String,
        url: Option<String>,
        #[serde(default)]
        summary: String,
        log: Option<String>,
    },
}

impl FeedbackItem {
    fn checkout_id(&self) -> &str {
        match self {
            Self::Thread { checkout_id, .. }
            | Self::Review { checkout_id, .. }
            | Self::Comment { checkout_id, .. }
            | Self::Check { checkout_id, .. } => checkout_id,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FeedbackNote {
    pub author: String,
    pub body: String,
}

/// A body as a Markdown quote, so a reviewer's own headings and lists stay
/// inside their comment instead of restructuring the file around it.
fn quoted(body: &str) -> String {
    body.trim()
        .lines()
        .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The feedback as a file an agent can work through.
///
/// `scope` is the checkout the agent is sitting in. Paths are qualified with
/// their repo everywhere else, so `api/src/auth.ts:42` is never ambiguous from
/// the task root.
pub(crate) fn feedback_markdown(
    branch: &str,
    items: &[FeedbackItem],
    scope: Option<&str>,
    repo_name: impl Fn(&str) -> String,
) -> String {
    let mut md = format!("# Feedback on the pull requests for `{branch}`\n\n");
    md.push_str(
        "Collected from GitHub by Layer. Quoted text is what reviewers and CI said; \
         none of it has been answered on GitHub yet.\n",
    );

    let mut order: Vec<&str> = Vec::new();
    for item in items {
        if !order.contains(&item.checkout_id()) {
            order.push(item.checkout_id());
        }
    }
    for checkout in order {
        let repo = repo_name(checkout);
        let mine: Vec<&FeedbackItem> = items.iter().filter(|i| i.checkout_id() == checkout).collect();
        md.push_str(&format!("\n## {repo}\n"));
        if scope.is_some_and(|s| s != checkout) {
            md.push_str(&format!("\nThis is a sibling repository: `../{repo}` from where you are.\n"));
        }

        let threads: Vec<_> = mine.iter().filter(|i| matches!(i, FeedbackItem::Thread { .. })).collect();
        if !threads.is_empty() {
            md.push_str("\n### Review threads\n");
            for item in threads {
                let FeedbackItem::Thread { path, line, start_line, code, outdated, resolved, url, comments, .. } = item
                else {
                    continue;
                };
                let at = if scope == Some(checkout) { path.clone() } else { format!("{repo}/{path}") };
                let at = match (start_line, line) {
                    (Some(a), Some(b)) if a < b => format!("{at}:{a}-{b}"),
                    (_, Some(n)) => format!("{at}:{n}"),
                    _ => at,
                };
                md.push_str(&format!("\n#### `{at}`\n"));
                if *resolved {
                    md.push_str("\nResolved on GitHub: the reviewer accepted it as it stands. It was sent on purpose, so read it, but do not undo what was settled.\n");
                }
                if *outdated {
                    md.push_str(
                        "\nThe code here has changed since this was written — check whether it still applies.\n",
                    );
                }
                if !code.is_empty() {
                    let diff: Vec<String> = code.iter().map(|c| format!("{}{}", c.op, c.text)).collect();
                    // Longer than any run of backticks in the code, so the code cannot close it.
                    let fence = "`".repeat(diff.iter().map(|l| l.matches('`').count()).max().unwrap_or(0).max(2) + 1);
                    md.push_str(&format!("\nThe code it was left on:\n\n{fence}diff\n{}\n{fence}\n", diff.join("\n")));
                }
                for c in comments {
                    md.push_str(&format!("\n**{}**:\n{}\n", c.author, quoted(&c.body)));
                }
                md.push_str(&format!("\n{url}\n"));
            }
        }

        let said: Vec<_> =
            mine.iter().filter(|i| matches!(i, FeedbackItem::Review { .. } | FeedbackItem::Comment { .. })).collect();
        if !said.is_empty() {
            md.push_str("\n### On the pull request as a whole\n");
            for item in said {
                let (author, verb, body, url) = match item {
                    FeedbackItem::Review { author, state, body, url, .. } => (
                        author,
                        match state.as_deref() {
                            Some("CHANGES_REQUESTED") => "requested changes",
                            Some("APPROVED") => "approved, adding",
                            _ => "reviewed",
                        },
                        body,
                        url,
                    ),
                    FeedbackItem::Comment { author, body, url, .. } => (author, "commented", body, url),
                    _ => continue,
                };
                md.push_str(&format!("\n**{author}** {verb}:\n{}\n\n{url}\n", quoted(body)));
            }
        }

        let checks: Vec<_> = mine.iter().filter(|i| matches!(i, FeedbackItem::Check { .. })).collect();
        if !checks.is_empty() {
            md.push_str("\n### Failing checks\n");
            for item in checks {
                let FeedbackItem::Check { name, conclusion, url, summary, log, .. } = item else { continue };
                md.push_str(&format!("\n#### {name} — {}\n", conclusion.replace('_', " ")));
                if let Some(url) = url {
                    md.push_str(&format!("\n{url}\n"));
                }
                if !summary.trim().is_empty() {
                    md.push_str(&format!("\nWhat the check reported:\n\n```\n{}\n```\n", summary.trim()));
                }
                match log {
                    Some(log) if !log.trim().is_empty() => md.push_str(&format!(
                        "\nThe end of its log, up to where it failed:\n\n```\n{}\n```\n",
                        log.trim()
                    )),
                    _ if summary.trim().is_empty() => {
                        md.push_str("\nNo log could be read — open the link, or run the same step locally.\n")
                    }
                    _ => {}
                }
            }
        }
    }
    md
}

/// "3 review threads, 1 comment and 2 failing checks".
fn feedback_summary(items: &[FeedbackItem]) -> String {
    let count = |f: fn(&FeedbackItem) -> bool| items.iter().filter(|i| f(i)).count();
    let parts: Vec<String> = [
        (count(|i| matches!(i, FeedbackItem::Thread { .. })), "review thread"),
        (count(|i| matches!(i, FeedbackItem::Review { .. } | FeedbackItem::Comment { .. })), "comment"),
        (count(|i| matches!(i, FeedbackItem::Check { .. })), "failing check"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}{}", if n == 1 { "" } else { "s" }))
    .collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Hand the chosen feedback to an agent.
///
/// Written to a file beside the worktrees and pointed at, not typed in: a
/// failing build's log is a hundred lines, and pasted into a TUI it arrives as
/// "[Pasted text +140 lines]" at best and as a hundred submissions at worst.
///
/// With `pane_id`, the prompt is typed into that agent. Without, it is only
/// returned, for starting a new agent with it — `scope` then says where that
/// agent will run.
#[tauri::command]
pub fn send_pr_feedback(
    state: State<AppState>,
    task_id: String,
    pane_id: Option<String>,
    scope: Option<String>,
    items: Vec<FeedbackItem>,
) -> Result<String> {
    if items.is_empty() {
        return Err(Error::Other("nothing chosen to send".into()));
    }
    let task = state.config.task(&task_id)?;
    let scope = match &pane_id {
        Some(id) => state.ptys.info(id)?.checkout_id,
        None => scope,
    };
    let md = feedback_markdown(&task.branch, &items, scope.as_deref(), |id| {
        state
            .config
            .checkout(id)
            .and_then(|c| state.config.project(&c.project_id))
            .map(|p| p.name)
            .unwrap_or_else(|_| "(unknown repo)".into())
    });

    let ask = "Work through every point. Fix what is right; where you think a reviewer is \
               wrong, say so and why instead of quietly skipping it. For a failing check, \
               find the cause before changing anything, and reproduce it locally if you can. \
               When you are done, go through the points one by one and say what you did \
               about each.";
    let head = format!(
        "Reviewers and CI have come back on the pull requests for `{}`: {}.",
        task.branch,
        feedback_summary(&items),
    );
    let prompt = match agent_file_dir(&state, &task) {
        Some(dir) => {
            let path = dir.join(FEEDBACK_FILE);
            // What to do goes in the file with the points: typed, the two
            // together were long enough to lose their start (`pty::MAX_TYPED`).
            std::fs::write(&path, format!("{head}\n\n{ask}\n\n{md}"))?;
            format!("{head} It is all in {}. Read that file, then work through every point.", path.display())
        }
        // A task from the one-repo layout has no folder outside its worktree,
        // and a file in the worktree is one the agent might commit.
        None => format!("{head}\n\n{md}\n\n{ask}"),
    };

    if let Some(id) = &pane_id {
        super::hand_over(&state, &task, id, FEEDBACK_FILE, &prompt)?;
    }
    Ok(prompt)
}

pub(crate) const FEEDBACK_FILE: &str = "PR_FEEDBACK.md";

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(checkout: &str, path: &str, line: Option<u64>) -> FeedbackItem {
        FeedbackItem::Thread {
            checkout_id: checkout.into(),
            path: path.into(),
            line,
            start_line: None,
            code: Vec::new(),
            outdated: false,
            resolved: false,
            url: "https://gh/t".into(),
            comments: vec![FeedbackNote { author: "ana".into(), body: "Off by one?\n\n- really".into() }],
        }
    }

    fn check(checkout: &str) -> FeedbackItem {
        FeedbackItem::Check {
            checkout_id: checkout.into(),
            name: "test".into(),
            conclusion: "timed_out".into(),
            url: None,
            summary: String::new(),
            log: Some("ERROR: boom".into()),
        }
    }

    fn row(state: Option<&str>, changed: usize) -> CheckoutPr {
        let pr = state.map(|s| github::PullRequest {
            id: None,
            number: 1,
            title: "t".into(),
            state: s.into(),
            draft: false,
            author: "me".into(),
            head: "b".into(),
            head_repo: None,
            head_sha: String::new(),
            base: "main".into(),
            url: String::new(),
            mergeable_state: None,
            merged: s == "closed",
            comments: 0,
            review_comments: 0,
        });
        CheckoutPr {
            checkout_id: "c".into(),
            repo: "api".into(),
            pin: None,
            pin_revision: "0".into(),
            pin_read: false,
            pr,
            checks: Vec::new(),
            reviews: Vec::new(),
            past: Vec::new(),
            verdict: "none".into(),
            base: "main".into(),
            changed,
            to_reply: 0,
            error: None,
            branch_covered: changed == 0,
            detached_unlanded: false,
            covered_merge: state == Some("closed"),
            action_target: None,
        }
    }

    #[test]
    fn a_task_is_settled_only_when_every_repo_is_done_with_its_pull_requests() {
        assert!(settled(&[row(Some("closed"), 3), row(None, 0)]), "merged, and a repo with nothing to open");
        assert!(!settled(&[row(Some("closed"), 0), row(Some("open"), 2)]), "one still in review");
        assert!(!settled(&[row(Some("closed"), 0), row(None, 4)]), "a repo with work and no PR yet");
        assert!(!settled(&[row(None, 0)]), "no PR at all: one may be opened any moment");
        let mut failed = row(Some("closed"), 0);
        failed.error = Some("rate limited".into());
        assert!(!settled(&[failed]), "what could not be read is not known to be done");
    }

    #[test]
    fn settled_does_not_ignore_other_open_pull_requests() {
        let mut closed = row(Some("closed"), 0);
        closed.past.push(row(Some("open"), 0).pr.unwrap());
        assert!(!settled(&[closed]));
    }

    #[test]
    fn a_settled_task_is_swept_again_only_after_a_quarter_of_an_hour() {
        let now = std::time::Instant::now();
        let ids = || vec!["open".to_string(), "done".to_string(), "done-long-ago".to_string()];
        let mut settled = HashMap::new();
        settled.insert("done".to_string(), now - std::time::Duration::from_secs(60));
        settled.insert("done-long-ago".to_string(), now - SETTLED_SWEEP);
        assert_eq!(due(ids(), &settled, now), vec!["open".to_string(), "done-long-ago".to_string()]);
    }

    fn name(id: &str) -> String {
        if id == "c1" {
            "api".into()
        } else {
            "web".into()
        }
    }

    #[test]
    fn paths_are_bare_only_inside_their_own_repo() {
        let items = vec![thread("c1", "src/a.ts", Some(42)), thread("c2", "src/b.ts", None)];

        let from_root = feedback_markdown("ACME-1", &items, None, name);
        assert!(from_root.contains("`api/src/a.ts:42`"));
        assert!(from_root.contains("`web/src/b.ts`"));

        let from_api = feedback_markdown("ACME-1", &items, Some("c1"), name);
        assert!(from_api.contains("`src/a.ts:42`"));
        assert!(from_api.contains("`web/src/b.ts`"));
        assert!(from_api.contains("`../web` from where you are"));
    }

    #[test]
    fn a_reviewers_markdown_stays_inside_the_quote() {
        let md = feedback_markdown("ACME-1", &[thread("c1", "a", Some(1))], None, name);
        assert!(md.contains("> Off by one?\n>\n> - really"));
    }

    #[test]
    fn a_thread_on_a_range_names_the_range_and_carries_its_code() {
        let mut item = thread("c1", "a.scss", Some(20));
        if let FeedbackItem::Thread { start_line, code, .. } = &mut item {
            *start_line = Some(19);
            *code = vec![
                CodeLine { n: Some(19), op: "+".into(), text: ".id {".into() },
                CodeLine { n: Some(20), op: "+".into(), text: "  content: \"```\";".into() },
            ];
        }
        let md = feedback_markdown("ACME-1", &[item], None, name);
        assert!(md.contains("#### `api/a.scss:19-20`"));
        // A fence the code's own backticks cannot close.
        assert!(md.contains("````diff\n+.id {\n+  content: \"```\";\n````"));
    }

    #[test]
    fn a_resolved_thread_sent_on_purpose_says_it_was_settled() {
        let mut item = thread("c1", "a.ts", Some(3));
        if let FeedbackItem::Thread { resolved, .. } = &mut item {
            *resolved = true;
        }
        assert!(feedback_markdown("ACME-1", &[item], None, name).contains("Resolved on GitHub"));
        assert!(!feedback_markdown("ACME-1", &[thread("c1", "a.ts", Some(3))], None, name).contains("Resolved"));
    }

    #[test]
    fn a_check_says_what_it_reported_and_where_it_failed() {
        let md = feedback_markdown("ACME-1", &[check("c1")], None, name);
        assert!(md.contains("#### test — timed out"));
        assert!(md.contains("```\nERROR: boom\n```"));
    }

    #[test]
    fn the_summary_counts_in_words() {
        assert_eq!(feedback_summary(&[check("c1")]), "1 failing check");
        assert_eq!(
            feedback_summary(&[thread("c1", "a", None), thread("c1", "b", None), check("c1")]),
            "2 review threads and 1 failing check"
        );
    }

    #[test]
    fn items_arrive_tagged_by_kind() {
        let items: Vec<FeedbackItem> = serde_json::from_value(serde_json::json!([
            { "kind": "comment", "checkout_id": "c1", "author": "bo", "body": "hi", "url": "u" },
            { "kind": "check", "checkout_id": "c1", "name": "lint", "conclusion": "failure", "url": null, "log": null }
        ]))
        .unwrap();
        assert!(matches!(items[0], FeedbackItem::Comment { .. }));
        assert!(matches!(items[1], FeedbackItem::Check { .. }));
    }
}
