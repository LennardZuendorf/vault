//! One configured PR read per checkout, with a scope that can be checked
//! again before publishing the result or acting on it.

use std::path::Path;
use std::time::Instant;

use crate::config::{AppConfig, Checkout, GithubConfig, Task, TrackerKind};
use crate::error::{Error, Result};
use crate::git;
use crate::integrations::github::{self, GitHub, PullRequest};
use crate::secrets;

use super::github::CheckoutPr;
use super::landed::{moves_to, revision_landed_here};
use super::pr_branches::{self, BranchStamp, HistoryScope, PullRead};
use super::pr_pins::{self, BoundGithub};
use super::{off_runtime, AppState};

mod apply;
mod target;

pub(super) use target::action_target;

pub(super) struct Connection {
    pub(super) config: GithubConfig,
    generation: u64,
    bound: Result<BoundGithub>,
    legacy: Option<GitHub>,
}

impl Connection {
    pub(super) async fn load(state: &AppState) -> Result<Self> {
        let connection = Self::load_with(state, secrets::github_credential).await?;
        if connection.credentials_unreadable() {
            Err(Error::GithubCredentialUnreadable)
        } else {
            Ok(connection)
        }
    }

    pub(super) async fn load_with(
        state: &AppState,
        credential: impl FnOnce() -> Result<secrets::GithubCredential> + Send + 'static,
    ) -> Result<Self> {
        // A snapshot of the settings and credential must use the same lock
        // order as Connect and pin writes.
        let (config, generation) = {
            let generation = state.config.tracker_credential_generation(TrackerKind::Github);
            let config = state.config.with(|c| c.github.clone()).ok_or(Error::NotConfigured("GitHub"))?;
            (config, *generation)
        };
        let cfg = config.clone();
        let connection = off_runtime(move || match credential() {
            Ok(credential) => Self::from_credential(cfg, generation, credential),
            Err(_) => Ok(Self { config: cfg, generation, bound: Err(Error::GithubCredentialUnreadable), legacy: None }),
        })
        .await??;
        let version = state.config.tracker_credential_generation(TrackerKind::Github);
        state.config.with(|c| connection.is_current(c, *version)).then_some(connection).ok_or_else(stale)
    }

    pub(super) fn credentials_unreadable(&self) -> bool {
        matches!(self.bound, Err(Error::GithubCredentialUnreadable))
    }

    fn from_credential(config: GithubConfig, generation: u64, credential: secrets::GithubCredential) -> Result<Self> {
        let legacy = credential.legacy_token().map(|token| GitHub::new(&config, token)).transpose()?;
        let bound = BoundGithub::with_credential(config.clone(), generation, credential);
        Ok(Self { config, generation, bound, legacy })
    }

    pub(super) fn client(&self) -> Result<&GitHub> {
        match &self.bound {
            Ok(bound) => Ok(bound.client()),
            Err(e) => self.legacy.as_ref().ok_or_else(|| Error::Other(e.to_string())),
        }
    }

    pub(super) fn validated_pr_link(&self, slug: &str, pr: &PullRequest) -> Result<String> {
        pr_pins::validated_pr_link(&self.config, slug, pr)
    }

    fn is_current(&self, cfg: &AppConfig, generation: u64) -> bool {
        generation == self.generation
            && cfg.github.as_ref().is_some_and(|g| g.api_url == self.config.api_url && g.web_url == self.config.web_url)
            && self.bound.as_ref().ok().is_none_or(|b| b.is_current(cfg, generation))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Local {
    slug: (String, String),
    branch: String,
    head: String,
    task_tip: Option<String>,
    remote_base: Option<String>,
    local_base: Option<String>,
    dirty: u32,
}

impl Local {
    fn read(checkout: &Checkout, task_branch: &str, config: &GithubConfig) -> Result<Self> {
        let slug = pr_branches::configured_checkout_repo(Path::new(&checkout.path), config)?;
        Self::at_repo(checkout, task_branch, slug)
    }

    fn at_repo(checkout: &Checkout, task_branch: &str, slug: (String, String)) -> Result<Self> {
        let path = Path::new(&checkout.path);
        let status = git::status(path)?;
        if status.head.is_empty() || status.branch.is_empty() {
            return Err(Error::Git("The checkout has no readable HEAD.".into()));
        }
        Ok(Self {
            slug,
            branch: if status.branch == "(detached)" { "HEAD".into() } else { status.branch },
            head: status.head,
            task_tip: git::task_branch_tip(path, task_branch),
            remote_base: git::remote_tip(path, &checkout.base),
            local_base: git::task_branch_tip(path, &checkout.base),
            dirty: status.dirty_files,
        })
    }
}

#[derive(Clone)]
struct CheckoutScope {
    checkout: Checkout,
    name: String,
    history: HistoryScope,
}

#[derive(Clone)]
pub(super) struct Scope {
    pub task: Task,
    checkouts: Vec<CheckoutScope>,
    tracker: Option<TrackerKind>,
    began: Instant,
}

impl Scope {
    pub(super) fn capture(state: &AppState, task_id: &str, began: Instant) -> Result<Self> {
        state.config.with(|config| {
            let task = config.tasks.iter().find(|t| t.id == task_id).ok_or_else(|| Error::NotFound("Task".into()))?;
            let checkouts = config
                .checkouts
                .iter()
                .filter(|c| c.task_id == task_id)
                .map(|checkout| {
                    let name = config
                        .projects
                        .iter()
                        .find(|p| p.id == checkout.project_id)
                        .map_or_else(|| "(unknown)".into(), |p| p.name.clone());
                    CheckoutScope {
                        checkout: checkout.clone(),
                        name,
                        history: HistoryScope::for_checkout(config, task, checkout),
                    }
                })
                .collect();
            Ok(Self { task: task.clone(), checkouts, tracker: config.tracker_in_use(), began })
        })
    }

    pub(super) fn checkout_ids(&self) -> impl Iterator<Item = &str> {
        self.checkouts.iter().map(|c| c.checkout.id.as_str())
    }

    pub(super) fn matches(&self, config: &AppConfig) -> bool {
        let Some(task) = config.tasks.iter().find(|t| t.id == self.task.id) else { return false };
        if task.branch != self.task.branch
            || task.root != self.task.root
            || task.issue_key != self.task.issue_key
            || task.issue_url != self.task.issue_url
            || config.tracker_in_use() != self.tracker
        {
            return false;
        }
        let current: Vec<_> = config.checkouts.iter().filter(|c| c.task_id == task.id).collect();
        current.len() == self.checkouts.len()
            && self.checkouts.iter().all(|before| {
                current.iter().any(|after| {
                    let a = &before.checkout;
                    a.id == after.id
                        && a.path == after.path
                        && a.project_id == after.project_id
                        && a.base == after.base
                        && a.base_commit == after.base_commit
                        && a.point_before_update == after.point_before_update
                        && a.pr_pin_revision == after.pr_pin_revision
                        && a.pr_pin == after.pr_pin
                        && before.history == HistoryScope::for_checkout(config, task, after)
                })
            })
    }

    fn accepts(&self, state: &AppState, config: &AppConfig, connection: &Connection, generation: u64) -> bool {
        connection.is_current(config, generation)
            && self.matches(config)
            && state.prs_read_at.lock().get(&self.task.id).is_none_or(|at| *at <= self.began)
    }

    pub(super) fn is_current(&self, state: &AppState, connection: &Connection) -> bool {
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        state.config.with(|config| self.accepts(state, config, connection, *generation))
    }

    pub(super) async fn checkout(&self, connection: &Connection, id: &str, details: bool) -> Result<Resolved> {
        let input = self
            .checkouts
            .iter()
            .find(|c| c.checkout.id == id)
            .ok_or_else(|| Error::NotFound(format!("checkout {id}")))?;
        Ok(resolve(connection, &self.task, input, details).await)
    }

    pub(super) async fn all(&self, connection: &Connection, details: bool) -> Result<Vec<Resolved>> {
        let mut rows = Vec::with_capacity(self.checkouts.len());
        for input in &self.checkouts {
            rows.push(resolve(connection, &self.task, input, details).await);
        }
        Ok(rows)
    }

    /// Recheck immediately before a destructive local action, on the blocking pool.
    pub(super) fn require_current(&self, state: &AppState, connection: &Connection, rows: &[Resolved]) -> Result<()> {
        for row in rows {
            row.check_local(connection, &self.task.branch)?;
        }
        self.is_current(state, connection).then_some(()).ok_or_else(stale)
    }

    pub(super) fn record_ticket_stage(
        &self,
        state: &AppState,
        connection: &Connection,
        stage: &str,
        tracker_current: impl FnOnce(&AppConfig) -> bool,
    ) -> Result<()> {
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        state.config.update_local_tasks_if(|config| {
            if !self.accepts(state, config, connection, *generation) || !tracker_current(config) {
                return Err(stale());
            }
            let Some(task) = config.tasks.iter_mut().find(|t| t.id == self.task.id) else {
                return Err(stale());
            };
            if task.ticket_stage != self.task.ticket_stage {
                return Ok(((), false));
            }
            task.ticket_stage = Some(stage.into());
            Ok(((), true))
        })
    }

    pub(super) fn follows_jira(&self) -> bool {
        self.tracker == Some(TrackerKind::Jira)
    }
}

/// Call under the GitHub credential lock, after the association is durable.
pub(super) fn invalidate(state: &AppState, task_id: &str) {
    state.prs_read_at.lock().insert(task_id.to_owned(), Instant::now());
    state.prs_settled.lock().remove(task_id);
    state.prs_seen.lock().remove(task_id);
}

pub(super) fn stale() -> Error {
    Error::Other("The task's pull request evidence changed during the read. Refresh and try again.".into())
}

#[derive(Clone)]
pub(super) struct Resolved {
    pub checkout: Checkout,
    pub row: CheckoutPr,
    slug: Option<(String, String)>,
    local: Option<Local>,
    branches: Option<BranchStamp>,
    advance: Option<String>,
    pub merged_heads: Vec<String>,
    pub(super) covered_by: Option<u64>,
    pub task_ref_landed: bool,
}

impl Resolved {
    pub(super) fn slug(&self) -> Result<(&str, &str)> {
        self.slug.as_ref().map(|l| (l.0.as_str(), l.1.as_str())).ok_or_else(|| {
            Error::Other(self.row.error.clone().unwrap_or_else(|| "The checkout repository could not be read.".into()))
        })
    }

    pub(super) fn require_complete(&self) -> Result<()> {
        match &self.row.error {
            Some(error) => Err(Error::Other(format!("{}: {error}", self.row.repo))),
            None => Ok(()),
        }
    }

    pub(super) fn check_local(&self, connection: &Connection, branch: &str) -> Result<()> {
        self.check_snapshot(&connection.config, branch)
    }

    fn check_snapshot(&self, config: &GithubConfig, branch: &str) -> Result<()> {
        let local = Local::read(&self.checkout, branch, config)?;
        (self.local.as_ref() == Some(&local)).then_some(()).ok_or_else(stale)
    }

    pub(super) fn publish_point(&self, task_branch: &str) -> Option<&str> {
        if self.local.as_ref().is_some_and(|l| l.branch == task_branch) {
            self.advance.as_deref().or(self.checkout.base_commit.as_deref())
        } else {
            self.checkout.base_commit.as_deref()
        }
    }

    pub(super) fn nothing_to_publish(&self) -> Result<String> {
        if self.row.changed > 0 {
            return Err(Error::Other("No changes committed on the task branch — commit them first.".into()));
        }
        Ok("no changes, skipped".into())
    }

    pub(super) fn publication(&self, task_branch: &str) -> Result<Publication<'_>> {
        self.require_complete()?;
        let (owner, name) = self.slug()?;
        Ok(match self.row.open() {
            Some(pr) if same_head(pr, task_branch, &format!("{owner}/{name}")) => Publication::Reuse(pr),
            Some(pr) => Publication::Alternate(pr),
            None => Publication::Create,
        })
    }
}

pub(super) enum Publication<'a> {
    Reuse(&'a PullRequest),
    Alternate(&'a PullRequest),
    Create,
}

async fn resolve(connection: &Connection, task: &Task, input: &CheckoutScope, details: bool) -> Resolved {
    let mut resolved = Resolved {
        checkout: input.checkout.clone(),
        row: CheckoutPr::unread(&input.checkout, &input.name),
        local: None,
        branches: None,
        slug: None,
        advance: None,
        merged_heads: Vec::new(),
        covered_by: None,
        task_ref_landed: false,
    };
    let checkout = input.checkout.clone();
    let branch = task.branch.clone();
    let history = input.history.clone();
    let cfg = connection.config.clone();
    let local = off_runtime(move || {
        let slug = pr_branches::configured_checkout_repo(Path::new(&checkout.path), &cfg)?;
        let local = Local::at_repo(&checkout, &branch, slug.clone());
        let evidence = pr_branches::branch_evidence(&checkout, &branch, &history);
        Ok::<_, Error>((slug, local, evidence))
    })
    .await
    .and_then(|read| read);
    let ((owner, name), local, evidence) = match local {
        Ok(ok) => ok,
        Err(error) => {
            resolved.row.error = Some(error.to_string());
            return resolved;
        }
    };
    let slug = format!("{owner}/{name}");
    resolved.branches = Some(evidence.stamp());
    let client = connection.client();
    let mut read = match &client {
        Ok(client) => pr_branches::read_pulls(client, &owner, &name, evidence).await,
        Err(e) => PullRead { prs: Vec::new(), errors: vec![Error::Other(e.to_string())] },
    };
    let automatic: Vec<_> = read.prs.iter().map(|p| p.number).collect();
    let verified = if let Some(saved) = &input.checkout.pr_pin {
        if let Ok(url) = pr_pins::display_pin(&connection.config, &slug, saved) {
            if let Some(pin) = &mut resolved.row.pin {
                pin.url = url;
            }
        }
        match &connection.bound {
            Ok(bound) => match pr_pins::read_pin(bound, (&owner, &name), saved).await {
                Ok((verified, pr)) => {
                    resolved.row.pin = Some(verified.pin);
                    resolved.row.pin_read = true;
                    Some(pr)
                }
                Err(e) => {
                    if let (Error::PrPinIdentityChanged, Some(pin)) = (&e, &mut resolved.row.pin) {
                        pin.url.clear();
                    }
                    read.errors.push(e);
                    None
                }
            },
            Err(e) => {
                if client.is_ok() {
                    read.errors.push(Error::Other(e.to_string()));
                }
                None
            }
        }
    } else {
        None
    };
    let pin = verified.as_ref().map(|p| p.number);
    order_pulls(&mut read.prs, verified);
    if let (true, Ok(client)) = (details, client) {
        enrich(client, &owner, &name, pin, &mut read, &mut resolved.row).await;
    }

    resolved.slug = Some((owner, name));
    let check = resolved.checkout.clone();
    let cfg = connection.config.clone();
    let branch = task.branch.clone();
    let merges = read.prs.iter().filter(|p| p.merged && valid_sha(&p.head_sha)).cloned().collect::<Vec<_>>();
    match off_runtime(move || {
        let local = local?;
        if local != Local::read(&check, &branch, &cfg)? {
            return Err(stale());
        }
        let facts = branch_facts(&check, &branch, &local, &slug, &merges, &automatic)?;
        Ok((facts, local))
    })
    .await
    .and_then(|r| r)
    {
        Ok((facts, local)) => {
            resolved.advance = facts.advance;
            resolved.row.changed = facts.changed;
            resolved.row.branch_covered = facts.branch_covered;
            resolved.row.detached_unlanded = local.branch == "HEAD" && !facts.branch_covered;
            resolved.row.covered_merge = facts.covered_by.is_some();
            resolved.covered_by = facts.covered_by;
            resolved.merged_heads = facts.merged_heads;
            resolved.task_ref_landed = facts.task_ref_landed;
            resolved.local = Some(local);
        }
        Err(e) => read.errors.push(e),
    };
    resolved.row.error = read.error_message();
    let mut prs = read.prs.into_iter();
    resolved.row.pr = prs.next();
    resolved.row.past = prs.collect();
    if resolved.row.error.is_none() {
        if let (Ok(slug), Some(pr)) = (resolved.slug(), resolved.row.open()) {
            resolved.row.action_target = action_target(connection, &resolved.checkout, slug, pr);
        }
    }
    resolved
}

fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn same_head(pr: &PullRequest, branch: &str, slug: &str) -> bool {
    pr.head == branch && pr.head_repo.as_ref().is_some_and(|repo| repo.eq_ignore_ascii_case(slug))
}

fn order_pulls(prs: &mut Vec<PullRequest>, verified: Option<PullRequest>) {
    let pinned = verified.as_ref().map(|p| p.number);
    if let Some(pr) = verified {
        prs.retain(|p| p.number != pr.number);
        prs.push(pr);
    }
    prs.sort_by_key(|p| std::cmp::Reverse((p.state == "open", Some(p.number) == pinned, p.number)));
}

async fn enrich(
    client: &GitHub,
    owner: &str,
    name: &str,
    pinned: Option<u64>,
    read: &mut PullRead,
    row: &mut CheckoutPr,
) {
    let Some(pr) = read.prs.first_mut().filter(|p| p.state == "open") else { return };
    let (detail, checks, reviews, replies) = tokio::join!(
        async {
            if Some(pr.number) == pinned {
                Ok(None)
            } else {
                client.pull(owner, name, pr.number).await.map(Some)
            }
        },
        client.checks(owner, name, &pr.head_sha),
        client.reviews(owner, name, pr.number),
        client.pr_to_reply(owner, name, pr.number),
    );
    match detail {
        Ok(Some(full))
            if full.number == pr.number
                && full.id == pr.id
                && full.head == pr.head
                && full.head_sha == pr.head_sha
                && full.state == pr.state
                && full.merged == pr.merged
                && full.head_repo == pr.head_repo =>
        {
            *pr = full
        }
        Ok(None) => {}
        Ok(Some(_)) => read.errors.push(stale()),
        Err(e) => read.errors.push(e),
    }
    row.checks = keep_part(checks, &mut read.errors);
    row.reviews = keep_part(reviews, &mut read.errors);
    row.to_reply = keep_part(replies, &mut read.errors);
    row.verdict = github::verdict(&row.reviews).into();
}

fn keep_part<T: Default>(result: Result<T>, errors: &mut Vec<Error>) -> T {
    result.unwrap_or_else(|error| {
        errors.push(error);
        T::default()
    })
}

struct BranchFacts {
    changed: usize,
    branch_covered: bool,
    covered_by: Option<u64>,
    merged_heads: Vec<String>,
    advance: Option<String>,
    task_ref_landed: bool,
}

fn branch_facts(
    checkout: &Checkout,
    task_branch: &str,
    local: &Local,
    slug: &str,
    merges: &[PullRequest],
    automatic: &[u64],
) -> Result<BranchFacts> {
    let path = Path::new(&checkout.path);
    let base = &checkout.base;
    let mut merges: Vec<_> = merges.iter().collect();
    merges.sort_by_key(|p| std::cmp::Reverse(p.number));
    let task_ref = format!("refs/heads/{task_branch}");
    let mut covers_task_tip = revision_landed_here(path, &task_ref, base, None);
    let mut covers_head = revision_landed_here(path, "HEAD", base, None);
    let mut covered_by = None;
    // Only the newest eligible merge can fetch an unknown object. A listing
    // can contain hundreds of old PRs; their tips must not fan out into Git
    // fetches on every sweep. Historical matching already requires a local SHA.
    let advance = checkout
        .point_before_update
        .is_none()
        .then(|| {
            merges
                .iter()
                .find(|pr| {
                    same_head(pr, &local.branch, slug)
                        && (local.branch == task_branch || automatic.contains(&pr.number))
                        && checkout.base_commit.as_deref() != Some(&pr.head_sha)
                })
                .and_then(|pr| moves_to(path, checkout.base_commit.as_deref(), base, &pr.head_sha))
        })
        .flatten();
    let mut merged_heads = Vec::new();
    for pr in merges {
        let sha = Some(pr.head_sha.as_str());
        let task_covered = local.task_tip.is_some() && revision_landed_here(path, &task_ref, base, sha);
        let head_covered = revision_landed_here(path, "HEAD", base, sha);
        // Checkout history makes an automatic merge relevant even after its
        // topic has been left. A pin alone doesn't: require it to cover work
        // not already on the base, or to match the checkout's own head by Git.
        let proves_merge = (automatic.contains(&pr.number) && (task_covered || head_covered))
            || (!covers_task_tip && task_covered)
            || (!covers_head && head_covered);
        if proves_merge && covered_by.is_none() {
            covered_by = Some(pr.number);
        }
        covers_task_tip |= task_covered;
        covers_head |= head_covered;
        if task_covered {
            merged_heads.push(pr.head_sha.clone());
        }
    }
    let changed = git::changed_count(path, base, advance.as_deref().or(checkout.base_commit.as_deref()))?
        .max(local.dirty as usize);
    Ok(BranchFacts {
        changed,
        branch_covered: (local.task_tip.is_none() || covers_task_tip) && covers_head,
        covered_by,
        merged_heads,
        advance,
        task_ref_landed: covers_task_tip,
    })
}

#[cfg(test)]
mod tests;
