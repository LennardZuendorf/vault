//! Tickets from the workspace's tracker (TKT-15): your queue, finding,
//! filing, and starting work on one. Whatever the tracker, through `Tracker`.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::{AppConfig, Task, TrackerKind};
use crate::error::{Error, Result};
use crate::tickets::github::{in_workspace, Issues};
use crate::tickets::{Find, NewTicket, RequiredField, Ticket, TicketPage, TicketType, Tracker};

use super::jira::{learn_epic_field, task_repos, ticket_prompt};
use super::panes::{start_agent, Resume};
use super::task_context::{keep_ticket, write_ticket_context};
use super::tasks::{check_new_ticket, new_task_from_ticket, ticket_owner, NewTask};
use super::AppState;

pub(crate) fn tracker(state: &AppState) -> Result<Tracker> {
    state.config.with(Tracker::connect)
}

/// Choose where the workspace's tickets come from (TKT-15). Admins only:
/// it is the workspace's, the same for everyone in it. Tasks keep the
/// tickets they have (TKT-16); the cached types were the old tracker's.
#[tauri::command]
pub fn set_tracker(state: State<AppState>, uses: TrackerKind) -> Result<()> {
    state.config.update(|c| {
        if c.shared.workspace.as_ref().is_some_and(|w| !w.is_admin()) {
            return Err(Error::Other("only the workspace's admins choose its tracker".into()));
        }
        c.tracker = Some(uses);
        Ok(())
    })??;
    *state.ticket_types.lock() = None;
    Ok(())
}

/// Every type of ticket the tracker defines, with its own icon. Nothing about
/// types is hardcoded — a site with custom types renders exactly as it does
/// in its tracker.
#[tauri::command]
pub async fn ticket_types(state: State<'_, AppState>, refresh: Option<bool>) -> Result<Vec<TicketType>> {
    ticket_types_inner(&state, refresh == Some(true)).await
}

pub(crate) async fn ticket_types_inner(state: &AppState, refresh: bool) -> Result<Vec<TicketType>> {
    if !refresh {
        if let Some(cached) = state.ticket_types.lock().clone() {
            return Ok(cached);
        }
    }
    let types = tracker(state)?.types().await?;
    *state.ticket_types.lock() = Some(types.clone());
    Ok(types)
}

/// The tracker's priorities, highest first, for sorting your list by them
/// (TKT-13). The UI asks once per connection.
#[tauri::command]
pub async fn ticket_priorities(state: State<'_, AppState>) -> Result<Vec<String>> {
    tracker(&state)?.priorities().await
}

#[tauri::command]
pub async fn tickets_mine(app: AppHandle, state: State<'_, AppState>) -> Result<TicketPage> {
    let page = my_tickets(&state).await?;
    crate::news::saw_tickets(&app, &page.issues);
    Ok(page)
}

/// Your queue, for the UI and for the watch that banners what is new.
pub(crate) async fn my_tickets(state: &AppState) -> Result<TicketPage> {
    if state.config.with(|c| c.tracker_in_use()) == Some(TrackerKind::Jira) {
        learn_epic_field(state).await;
    }
    tracker(state)?.mine().await
}

/// Look past your own queue: unassigned work, or anyone else's.
#[tauri::command]
pub async fn tickets_find(
    app: AppHandle,
    text: Option<String>,
    whose: String,
    include_done: Option<bool>,
    types: Option<Vec<String>>,
    order: Option<String>,
    for_link: Option<bool>,
) -> Result<TicketSearch> {
    let find = Find {
        text,
        whose,
        include_done: include_done.unwrap_or(false),
        types: types.unwrap_or_default(),
        order: order.unwrap_or_default(),
    };
    if for_link == Some(true) {
        find_tickets_for_link_with(app, find, connect_for_link).await
    } else {
        let page = tracker(&app.state::<AppState>())?.find(find).await?;
        Ok(TicketSearch { page, tracker_scope: None })
    }
}

#[derive(Debug, Serialize)]
pub struct TicketSearch {
    #[serde(flatten)]
    page: TicketPage,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracker_scope: Option<String>,
}

async fn find_tickets_for_link_with<R: tauri::Runtime>(
    app: AppHandle<R>,
    find: Find,
    connect: impl FnOnce(&AppConfig) -> Result<Tracker> + Send + 'static,
) -> Result<TicketSearch> {
    let (tracker, scope) =
        super::blocking(app.clone(), move |state| with_tracker_scope(state, |cfg, scope| Ok((connect(cfg)?, scope))))
            .await?;
    let page = tracker.find(find).await?;
    super::blocking(app, move |state| {
        with_tracker_scope(state, |_, current| {
            if current != scope {
                return Err(picker_connection_changed());
            }
            Ok(TicketSearch { page, tracker_scope: Some(scope.token()?) })
        })
    })
    .await
}

/// These tickets, wherever they are, for the epic each belongs to (TASK-17,
/// REV-11). A task's ticket or a PR's is often not on your own list: done,
/// given to someone else, or never yours.
#[tauri::command]
pub async fn tickets_by_key(state: State<'_, AppState>, keys: Vec<String>) -> Result<Vec<Ticket>> {
    tracker(&state)?.by_key(&keys).await
}

/// Whatever this tracker calls an ordinary ticket.
///
/// Not "Task": that is one site's name for it. Hierarchy level means the same
/// everywhere — 0 is a standard ticket — so the plain type is found rather
/// than assumed, preferring one actually called Task when there is one.
pub(crate) async fn default_ticket_type(state: &AppState) -> Result<String> {
    let types = ticket_types_inner(state, false).await?;
    let plain: Vec<&TicketType> = types.iter().filter(|t| t.hierarchy_level == 0 && !t.subtask).collect();
    plain
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("task"))
        .or_else(|| plain.first())
        .map(|t| t.name.clone())
        .ok_or_else(|| Error::Other("this tracker defines no ordinary type of ticket".into()))
}

/// What a project demands before it will accept a new ticket of this type.
#[tauri::command]
pub async fn ticket_required_fields(
    state: State<'_, AppState>,
    project_key: String,
    issue_type_id: String,
) -> Result<Vec<RequiredField>> {
    required_fields_for(&state, &project_key, &issue_type_id).await
}

/// `required_fields`, for a type id that may belong to another project.
///
/// The type list is one entry per name, but a team-managed project has its
/// own "Task" with an id of its own, so the id picked from that list is often
/// some other project's — and Jira answers it with a 404. When it does, the
/// same name is looked up in the project being filed into.
pub(crate) async fn required_fields_for(
    state: &AppState,
    project_key: &str,
    issue_type_id: &str,
) -> Result<Vec<RequiredField>> {
    let tracker = tracker(state)?;
    let first = tracker.required_fields(project_key, issue_type_id).await;
    if first.is_ok() {
        return first;
    }
    let name = ticket_types_inner(state, false)
        .await
        .ok()
        .and_then(|types| types.into_iter().find(|t| t.id == issue_type_id).map(|t| t.name));
    let Some(name) = name else {
        return first;
    };
    match tracker.type_in_project(project_key, &name, issue_type_id).await {
        Some(id) => tracker.required_fields(project_key, &id).await,
        None => first,
    }
}

/// File a ticket without starting work on it.
///
/// Filing under an epic and opening worktrees are different intentions:
/// adding a ticket to a plan is not saying you will start it now. Start work
/// on the ticket does that (`start_work_on_ticket`).
#[tauri::command]
pub async fn file_ticket(state: State<'_, AppState>, req: NewTicket) -> Result<Ticket> {
    if req.summary.trim().is_empty() {
        return Err(Error::Other("the ticket needs a summary".into()));
    }
    let tracker = tracker(&state)?;
    let key = tracker.file(&req).await?;
    tracker.ticket(&key).await
}

#[derive(Debug, Serialize)]
pub struct LinkedTaskTicket {
    pub task: Task,
    pub ticket: Ticket,
    pub context_warning: Option<String>,
}

/// Associate an existing ticket with this task without replaying Start work.
#[tauri::command]
pub async fn link_task_ticket(
    app: AppHandle,
    task_id: String,
    key: String,
    tracker_scope: String,
) -> Result<LinkedTaskTicket> {
    link_task_ticket_with(app, task_id, key, Some(tracker_scope), connect_for_link).await
}

#[derive(Clone, Serialize)]
struct TaskTicketLinked<'a> {
    task_id: &'a str,
    key: &'a str,
    url: &'a str,
    status: &'a str,
    status_category: &'a str,
    context_warning: Option<&'a str>,
}

pub(crate) async fn link_task_ticket_inner(app: AppHandle, task_id: String, key: String) -> Result<LinkedTaskTicket> {
    link_task_ticket_with(app, task_id, key, None, connect_for_link).await
}

fn connect_for_link(cfg: &AppConfig) -> Result<Tracker> {
    connect_for_link_with(cfg, crate::secrets::github_credential)
}

fn connect_for_link_with(
    cfg: &AppConfig,
    stored: impl FnOnce() -> Result<crate::secrets::GithubCredential>,
) -> Result<Tracker> {
    if cfg.tracker_in_use() != Some(TrackerKind::Github) {
        return Tracker::connect(cfg);
    }
    let config = cfg.github.as_ref().ok_or(Error::NotConfigured("GitHub"))?;
    let origin = super::pr_pins::api_origin(config)?;
    let token = stored()?.for_origin(&origin)?;
    let client = crate::integrations::github::GitHub::new(config, &token)?;
    Ok(Tracker::Github(client, Issues::of(cfg, &config.web_url)))
}

async fn link_task_ticket_with<R: tauri::Runtime>(
    app: AppHandle<R>,
    task_id: String,
    key: String,
    expected_scope: Option<String>,
    connect: impl FnOnce(&AppConfig) -> Result<Tracker> + Send + 'static,
) -> Result<LinkedTaskTicket> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(Error::Other("Choose a ticket to link.".into()));
    }
    let (tracker, scope, task_id, key) = super::blocking(app.clone(), move |state| {
        with_tracker_scope(state, |c, scope| {
            linkable_task(c, &task_id)?;
            if let Some(expected) = expected_scope.as_deref() {
                if expected != scope.token()? {
                    return Err(picker_connection_changed());
                }
            }
            let tracker = connect(c)?;
            if let Tracker::Github(_, issues) = &tracker {
                in_workspace(issues, &key)?;
            }
            Ok((tracker, scope, task_id, key))
        })
    })
    .await?;
    let ticket = tracker.ticket(&key).await?;
    if ticket.key.trim().is_empty() || ticket.url.trim().is_empty() {
        return Err(Error::Other("The tracker did not return a ticket key and URL. Try again.".into()));
    }

    let linked = super::blocking(app.clone(), move |state| {
        // The lookup did not hold the config lock. Recheck before claiming the
        // ticket so concurrent calls cannot replace a link or both claim it.
        let TrackerScope { identity, version } = scope;
        let task = {
            let generation = state.config.tracker_credential_generation(identity.0);
            if *generation != version {
                return Err(tracker_changed());
            }
            state.config.update_local_tasks(|c| {
                if tracker_identity(c).as_ref() != Some(&identity) {
                    return Err(tracker_changed());
                }
                if let (TrackerKind::Github, _, web_url) = &identity {
                    let issues = Issues::of(c, web_url);
                    in_workspace(&issues, &key)?;
                    in_workspace(&issues, &ticket.key)?;
                }
                let at = linkable_task(c, &task_id)?;
                if let Some(owner) = ticket_owner(&c.tasks, &ticket.key) {
                    return Err(Error::Other(format!(
                        "{} is already linked to the task \"{}\".",
                        ticket.key, c.tasks[owner].name
                    )));
                }
                let task = &mut c.tasks[at];
                task.issue_key = Some(ticket.key.clone());
                task.issue_url = Some(ticket.url.clone());
                Ok(task.clone())
            })?
        };
        let context_warning = write_ticket_context(state, &task, &ticket)
            .err()
            .map(|e| format!("The ticket is linked, but Layer could not refresh the agent context: {e}"));
        Ok(LinkedTaskTicket { task, ticket, context_warning })
    })
    .await?;
    // Both entry points report the saved link, even when writing context failed.
    let _ = app.emit(
        "task:ticket-linked",
        TaskTicketLinked {
            task_id: &linked.task.id,
            key: &linked.ticket.key,
            url: &linked.ticket.url,
            status: &linked.ticket.status,
            status_category: &linked.ticket.status_category,
            context_warning: linked.context_warning.as_deref(),
        },
    );
    Ok(linked)
}

fn tracker_changed() -> Error {
    Error::Other("The tracker connection changed during lookup. Try again.".into())
}

fn picker_connection_changed() -> Error {
    Error::Other("The tracker connection changed. Search for the ticket again.".into())
}

#[derive(PartialEq, Serialize)]
struct TrackerScope {
    identity: (TrackerKind, String, String),
    version: u64,
}

impl TrackerScope {
    fn token(&self) -> Result<String> {
        // The webview echoes this process's proof, not a site/account it chose.
        // Only identity and generation are hashed, never any credential.
        static NONCE: LazyLock<[u8; 16]> = LazyLock::new(|| *uuid::Uuid::new_v4().as_bytes());
        let mut hash = Sha256::new();
        hash.update(*NONCE);
        hash.update(serde_json::to_vec(self)?);
        Ok(hex::encode(hash.finalize()))
    }
}

fn with_tracker_scope<T>(state: &AppState, read: impl FnOnce(&AppConfig, TrackerScope) -> Result<T>) -> Result<T> {
    let kind = state.config.with(AppConfig::tracker_in_use).ok_or(Error::NotConfigured("Ticket tracker"))?;
    let generation = state.config.tracker_credential_generation(kind);
    let config = state.config.read();
    let identity = tracker_identity(&config).ok_or(Error::NotConfigured("Ticket tracker"))?;
    if identity.0 != kind {
        return Err(tracker_changed());
    }
    read(&config, TrackerScope { identity, version: *generation })
}

// Search preferences and workflow destinations may change without replacing
// the site or account the ticket was resolved against.
fn tracker_identity(c: &AppConfig) -> Option<(TrackerKind, String, String)> {
    let kind = c.tracker_in_use()?;
    match kind {
        TrackerKind::Jira => c.jira.as_ref().map(|j| (kind, j.base_url.clone(), j.email.clone())),
        TrackerKind::Github => c.github.as_ref().map(|g| (kind, g.api_url.clone(), g.web_url.clone())),
    }
}

pub(super) fn jira_ticket_scope(cfg: &crate::config::JiraConfig, version: u64, task: &Task) -> Result<String> {
    let scope = TrackerScope { identity: (TrackerKind::Jira, cfg.base_url.clone(), cfg.email.clone()), version };
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(&(scope.token()?, &task.id, &task.issue_url))?);
    Ok(hex::encode(hash.finalize()))
}

fn linkable_task(c: &AppConfig, id: &str) -> Result<usize> {
    let at = c.tasks.iter().position(|t| t.id == id).ok_or_else(|| Error::NotFound(format!("task {id}")))?;
    let task = &c.tasks[at];
    if task.epic || task.review.is_some() || task.sub_task.is_some() {
        return Err(Error::Other("Only regular Work tasks can link a ticket.".into()));
    }
    if task.issue_key.is_some() || task.issue_url.is_some() {
        return Err(Error::Other("This task already has a ticket. Open its linked ticket instead.".into()));
    }
    Ok(at)
}

/// A task, and the status its ticket was moved to on the way.
#[derive(Debug, Serialize)]
pub struct Started {
    #[serde(flatten)]
    pub task: Task,
    /// The status the ticket was moved to, when it was moved.
    pub moved: Option<String>,
}

/// The one-click path: ticket -> worktree per repo -> agent primed with both
/// the ticket and the layout.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_work_on_ticket(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    project_ids: Vec<String>,
    agent_id: Option<String>,
    branch_suffix: Option<String>,
    base: Option<String>,
    epic: Option<bool>,
) -> Result<Started> {
    let epic = epic.unwrap_or(false);
    // Fast refusal for a known key; the insertion rechecks the resolved key.
    state.config.with(|c| check_new_ticket(&c.tasks, &key, branch_suffix.as_deref()))?;

    let tracker = tracker(&state)?;
    let issue = tracker.ticket(&key).await?;

    let new = NewTask {
        name: format!("{} {}", issue.key, issue.summary),
        project_ids,
        branch: None,
        branch_suffix,
        base,
        issue_key: Some(issue.key.clone()),
        issue_url: Some(issue.url.clone()),
        epic_key: issue.epic_key.clone(),
        epic,
        sub_task: None,
    };
    // Worktrees on the blocking pool, as in `create_task`.
    let task = super::blocking(app.clone(), move |state| new_task_from_ticket(state, new)).await?;
    keep_ticket(app.clone(), task.clone(), issue.clone()).await;

    // An epic's work is done in its sub-tasks: no agent starts in it, and
    // its ticket moves when the first of them starts (EPIC-2).
    if epic {
        return Ok(Started { task, moved: None });
    }
    if let Some(agent_id) = agent_id {
        let repos = task_repos(&state, &task);
        let prompt = ticket_prompt(&issue, &task, &repos, true);
        let task_id = task.id.clone();
        let handle = app.clone();
        super::blocking(app.clone(), move |state| {
            start_agent(&handle, state, task_id, agent_id, None, Some(prompt), Resume::No, None, None, false)
        })
        .await?;
    }

    // Best effort, as Jira's move is: the task is made either way.
    if let Err(e) = tracker.take(&issue).await {
        eprintln!("could not give {key} to you as work started: {e}");
    }
    let moved = super::ticket_flow::started(&state, &issue.key).await;
    Ok(Started { task, moved })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GithubConfig, JiraConfig};
    use crate::integrations::{github::GitHub, jira::Jira};
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tauri::test::{mock_app, MockRuntime};
    use tauri::Manager;
    use tokio::net::TcpListener;

    use super::super::repos::tests::{add_task, git, setup, state};

    struct Fixture {
        app: tauri::App<MockRuntime>,
        root: PathBuf,
        listener: Option<TcpListener>,
        url: String,
    }

    impl Fixture {
        async fn new(kind: TrackerKind) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (root, mut cfg) = setup();
            cfg.tracker = Some(kind);
            cfg.jira =
                Some(JiraConfig { base_url: url.clone(), email: "dev@example.test".into(), ..Default::default() });
            cfg.github = Some(GithubConfig {
                api_url: url.clone(),
                web_url: "https://github.example.test".into(),
                ..Default::default()
            });
            cfg.projects[0].remote = Some("github.example.test/acme/api".into());
            add_task(&mut cfg, &root, "first");
            add_task(&mut cfg, &root, "second");
            for task in &cfg.tasks {
                std::fs::create_dir_all(&task.root).unwrap();
            }
            let app = mock_app();
            app.manage(state(&root, cfg));
            Self { app, root, listener: Some(listener), url }
        }

        fn serve(&mut self, router: Router) {
            let listener = self.listener.take().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        }

        fn serve_jira(&mut self) {
            self.serve(Router::new().route("/rest/api/3/issue/{key}", get(|| async { jira_answer() })));
        }

        fn handle(&self) -> AppHandle<MockRuntime> {
            self.app.handle().clone()
        }

        fn state(&self) -> tauri::State<'_, AppState> {
            self.app.state()
        }

        async fn link(&self, id: &str, key: &str) -> Result<Value> {
            Ok(serde_json::to_value(link_task_ticket_with(self.handle(), id.into(), key.into(), None, connect).await?)?)
        }

        fn task_root(&self, id: &str) -> PathBuf {
            PathBuf::from(self.state().config.task(id).unwrap().root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    // Keep the actual tracker clients and their response decoding. Only the
    // token source and the remote service are test fixtures.
    fn connect(cfg: &AppConfig) -> Result<Tracker> {
        match cfg.tracker_in_use() {
            Some(TrackerKind::Jira) => {
                let jira = cfg.jira.clone().unwrap();
                Ok(Tracker::Jira(Jira::new(&jira, "test-token")?, jira))
            }
            Some(TrackerKind::Github) => {
                let gh = cfg.github.as_ref().unwrap();
                Ok(Tracker::Github(
                    GitHub::new(gh, "test-token")?,
                    crate::tickets::github::Issues::of(cfg, &gh.web_url),
                ))
            }
            None => Err(Error::NotConfigured("Ticket tracker")),
        }
    }

    fn jira_answer() -> Json<Value> {
        Json(json!({
            "key": "API-42",
            "fields": {
                "summary": "Repair the API",
                "description": "Someone else's ticket",
                "assignee": { "displayName": "Ada" },
                "status": { "name": "To Do", "id": "11", "statusCategory": { "key": "new" } },
                "issuetype": { "name": "Bug" }
            }
        }))
    }

    fn github_answer() -> Json<Value> {
        Json(json!({ "data": { "i0": { "issue": {
            "number": 42, "title": "Repair the API", "body": "A description", "state": "OPEN",
            "url": "https://github.example.test/Acme/Api/issues/42",
            "repository": { "nameWithOwner": "Acme/Api" },
            "assignees": { "nodes": [] }, "labels": { "nodes": [] }, "parent": null,
            "subIssuesSummary": { "total": 0 }, "createdAt": "2026-10-01T12:00:00Z",
            "updatedAt": "2026-10-02T12:00:00Z"
        } } } }))
    }

    #[tokio::test]
    async fn linking_keeps_the_task_checkout_and_spec_and_persists_the_canonical_ticket() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve(Router::new().route("/rest/api/3/issue/old-42", get(|| async { jira_answer() })));
        let root = f.task_root("first");
        let cfg = f.state().config.read();
        let checkout = root.join("api");
        git(Path::new(&cfg.projects[0].path), &["worktree", "add", "-qb", "first", checkout.to_str().unwrap()]);
        let spec = checkout.join("specs/first/requirements.md");
        std::fs::create_dir_all(spec.parent().unwrap()).unwrap();
        let requirements = "# Goal\nKeep the original spec.";
        std::fs::write(&spec, requirements).unwrap();
        let head = git(&checkout, &["rev-parse", "HEAD"]);
        let status = git(&checkout, &["status", "--porcelain"]);

        let result = f.link("first", "old-42").await.unwrap();

        let url = format!("{}/browse/API-42", f.url);
        assert_eq!(result["ticket"]["key"], "API-42");
        assert_eq!(result["ticket"]["assignee"], "Ada");
        assert_eq!(result["ticket"]["url"], url);
        assert!(result["context_warning"].is_null());
        let mut expected = serde_json::to_value(&cfg.tasks[0]).unwrap();
        expected["issue_key"] = json!("API-42");
        expected["issue_url"] = json!(url);
        assert_eq!(result["task"], expected);
        let path = f.state().config.folder().join("config.json");
        let on_disk: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(on_disk["tasks"][0], expected);
        assert_eq!(on_disk["checkouts"], serde_json::to_value(cfg.checkouts).unwrap());
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&checkout, &["status", "--porcelain"]), status);
        assert_eq!(std::fs::read_to_string(spec).unwrap(), requirements);
        assert!(std::fs::read_to_string(root.join("TICKET.md")).unwrap().contains("API-42 Repair the API"));
        for file in ["AGENTS.md", "CLAUDE.md"] {
            let context = std::fs::read_to_string(root.join(file)).unwrap();
            assert!(context.contains("Keep the original spec."));
            assert!(context.contains("API-42 Repair the API"));
        }
    }

    fn picker_find() -> Find {
        Find {
            text: Some("API-42".into()),
            whose: "anyone".into(),
            include_done: true,
            types: vec![],
            order: String::new(),
        }
    }

    fn picker_answer() -> Json<Value> {
        Json(json!({ "issues": [jira_answer().0] }))
    }

    #[tokio::test]
    async fn github_link_paths_do_not_send_an_unbound_token_to_the_configured_api() {
        let mut f = Fixture::new(TrackerKind::Github).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        f.serve(Router::new().fallback(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                github_answer()
            }
        }));

        for origin in [Some("https://old-api.example.test"), None] {
            let connect = move |cfg: &AppConfig| {
                connect_for_link_with(cfg, || Ok(crate::secrets::GithubCredential::fixture("fixture", origin)))
            };
            let search = find_tickets_for_link_with(f.handle(), picker_find(), connect).await.unwrap_err();
            let link = link_task_ticket_with(f.handle(), "first".into(), "acme/api#42".into(), None, connect)
                .await
                .unwrap_err();
            for err in [search, link] {
                assert!(err.to_string().contains("enter the token again"), "{err}");
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
    }

    #[tokio::test]
    async fn picker_rejects_the_same_key_after_the_backend_switches_jira_sites() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        f.serve(Router::new().route("/rest/api/3/search/jql", post(|| async { picker_answer() })).route(
            "/other/rest/api/3/issue/API-42",
            get(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    jira_answer()
                }
            }),
        ));
        let page = find_tickets_for_link_with(f.handle(), picker_find(), connect).await.unwrap();
        assert_eq!(page.page.issues[0].url, format!("{}/browse/API-42", f.url));
        f.state().config.update(|c| c.jira.as_mut().unwrap().base_url = format!("{}/other", f.url)).unwrap();

        let result =
            link_task_ticket_with(f.handle(), "first".into(), "API-42".into(), page.tracker_scope, connect).await;

        assert!(result.unwrap_err().to_string().contains("connection changed"));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the new connection must not be asked for the old result");
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
    }

    #[tokio::test]
    async fn picker_requires_a_new_search_after_a_same_site_credential_change() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve(
            Router::new()
                .route("/rest/api/3/search/jql", post(|| async { picker_answer() }))
                .route("/rest/api/3/issue/API-42", get(|| async { jira_answer() })),
        );
        let page = find_tickets_for_link_with(f.handle(), picker_find(), connect).await.unwrap();
        *f.state().config.tracker_credential_generation(TrackerKind::Jira) += 1;

        let stale =
            link_task_ticket_with(f.handle(), "first".into(), "API-42".into(), page.tracker_scope, connect).await;
        assert!(stale.unwrap_err().to_string().contains("connection changed"));
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());

        let fresh = find_tickets_for_link_with(f.handle(), picker_find(), connect).await.unwrap();
        let linked = link_task_ticket_with(f.handle(), "first".into(), "API-42".into(), fresh.tracker_scope, connect)
            .await
            .unwrap();
        assert_eq!(linked.task.issue_key.as_deref(), Some("API-42"));
    }

    #[tokio::test]
    async fn picker_does_not_return_search_results_if_the_connection_changed_in_flight() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        let app = f.handle();
        f.serve(Router::new().route(
            "/rest/api/3/search/jql",
            post(move || {
                let app = app.clone();
                async move {
                    app.state::<AppState>()
                        .config
                        .update(|c| c.jira.as_mut().unwrap().email = "new@example.test".into())
                        .unwrap();
                    picker_answer()
                }
            }),
        ));

        let result = find_tickets_for_link_with(f.handle(), picker_find(), connect).await;

        assert!(result.unwrap_err().to_string().contains("connection changed"));
    }

    #[tokio::test]
    async fn linking_refuses_an_existing_link_and_does_not_touch_its_context() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        f.state()
            .config
            .update(|c| {
                c.tasks[0].issue_key = Some("API-9".into());
                c.tasks[0].issue_url = Some("https://example.test/browse/API-9".into());
            })
            .unwrap();
        let path = f.task_root("first").join("TICKET.md");
        std::fs::write(&path, "previously linked").unwrap();

        let err = f.link("first", "API-42").await.unwrap_err();

        assert!(err.to_string().contains("already has a ticket"), "{err}");
        assert_eq!(f.state().config.task("first").unwrap().issue_key.as_deref(), Some("API-9"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "previously linked");
    }

    #[tokio::test]
    async fn linking_rejects_a_canonical_ticket_owned_by_another_task() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        f.state().config.update(|c| c.tasks[1].issue_key = Some("API-42".into())).unwrap();

        let err = f.link("first", "old-42").await.unwrap_err();

        assert!(err.to_string().contains("already linked to"), "{err}");
        assert!(err.to_string().contains("second"), "{err}");
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
        assert_eq!(f.state().config.task("second").unwrap().issue_key.as_deref(), Some("API-42"));
    }

    #[tokio::test]
    async fn a_link_whose_config_save_fails_does_not_claim_the_ticket() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        let path = f.state().config.folder().join("config.json");
        std::fs::create_dir(&path).unwrap();

        assert!(matches!(f.link("first", "API-42").await, Err(Error::Io(_))));
        let task = f.state().config.task("first").unwrap();
        assert!(task.issue_key.is_none() && task.issue_url.is_none(), "the failed save claimed the ticket");
        assert!(!f.task_root("first").join("TICKET.md").exists());

        std::fs::remove_dir(&path).unwrap();
        f.state().config.update(|c| c.tasks[1].name = "Renamed".into()).unwrap();
        let on_disk: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(on_disk["tasks"][0]["issue_key"].is_null());
        assert_eq!(f.link("first", "API-42").await.unwrap()["task"]["issue_key"], "API-42");
    }

    #[tokio::test]
    async fn a_lookup_from_before_a_same_site_github_reconnect_cannot_link() {
        let mut f = Fixture::new(TrackerKind::Github).await;
        let app = f.handle();
        let url = f.url.clone();
        f.serve(Router::new().route("/user", get(|| async { Json(json!({ "login": "other-account" })) })).route(
            "/graphql",
            post(move |headers: axum::http::HeaderMap| {
                let (app, url) = (app.clone(), url.clone());
                async move {
                    assert_eq!(headers["authorization"], "Bearer test-token");
                    let login = super::super::github::github_connect(
                        app,
                        url,
                        "https://github.example.test".into(),
                        "other-test-token".into(),
                        None,
                    )
                    .await
                    .unwrap();
                    assert_eq!(login, "other-account");
                    github_answer()
                }
            }),
        ));

        let result = f.link("first", "acme/api#42").await;
        crate::secrets::delete(crate::secrets::GITHUB).unwrap();

        assert!(result.unwrap_err().to_string().contains("connection changed"));
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
        assert!(!f.task_root("first").join("TICKET.md").exists());
    }

    #[tokio::test]
    async fn starting_work_after_a_lookup_cannot_duplicate_a_newly_linked_ticket_without_a_suffix() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        // Start work already resolved the ticket, but the other task wins
        // the claim before Start work reaches the creation boundary.
        let req = |branch_suffix| NewTask {
            name: "Repair the API".into(),
            project_ids: vec!["api".into()],
            branch: None,
            branch_suffix,
            base: None,
            issue_key: Some("API-42".into()),
            issue_url: Some(format!("{}/browse/API-42", f.url)),
            epic_key: None,
            epic: false,
            sub_task: None,
        };
        f.link("first", "old-42").await.unwrap();
        let state = f.state();
        let new_root = state.config.worktree_root().join("API-42");

        let result = new_task_from_ticket(&state, req(None));

        let err = result.unwrap_err().to_string();
        assert!(err.contains("already has a task on branch first"), "{err}");
        assert_eq!(state.config.read().tasks.len(), 2);
        assert!(!new_root.exists());
        let task = new_task_from_ticket(&state, req(Some("second-pass".into()))).unwrap();
        assert_eq!(task.branch, "API-42-second-pass");
        assert_eq!(task.issue_key.as_deref(), Some("API-42"));
    }

    #[tokio::test]
    async fn an_agent_context_refresh_uses_the_link_even_when_its_caller_read_the_old_task() {
        let f = Fixture::new(TrackerKind::Jira).await;
        let state = f.state();
        let task = state.config.task("first").unwrap();
        let url = format!("{}/browse/API-42", f.url);
        // The association is durable, but the ticket file has not been
        // refreshed yet. The stale caller must still render the saved link.
        state
            .config
            .update(|c| {
                c.tasks[0].issue_key = Some("API-42".into());
                c.tasks[0].issue_url = Some(url.clone());
            })
            .unwrap();

        super::super::task_context::write_task_context(&state, &task).unwrap();

        for file in ["AGENTS.md", "CLAUDE.md"] {
            let context = std::fs::read_to_string(Path::new(&task.root).join(file)).unwrap();
            assert!(context.contains(&format!("Ticket: {url}")), "{file} dropped the saved ticket");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_older_context_writer_cannot_overwrite_the_linked_ticket() {
        use std::os::unix::fs::OpenOptionsExt;
        use std::time::Duration;

        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        let root = f.task_root("first");
        // Pause context rendering at the next file read after TICKET.md.
        let fifo = root.join(".spec-drafts/approved/requirements.md");
        std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let old = f.state().config.task("first").unwrap();
        let older = tokio::spawn(super::super::blocking(f.handle(), move |state| {
            super::super::task_context::write_task_context(state, &old)
        }));
        let pipe = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo) {
                    Ok(pipe) => break pipe,
                    Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(e) => panic!("opening the spec FIFO: {e}"),
                }
            }
        })
        .await
        .expect("the older writer reads the spec after TICKET.md");
        // Only the older writer will block on the already open pipe.
        std::fs::remove_file(&fifo).unwrap();
        let mut link = std::pin::pin!(f.link("first", "API-42"));
        let early = tokio::time::timeout(Duration::from_secs(2), link.as_mut()).await;
        drop(pipe);
        older.await.unwrap().unwrap();
        let linked = match early {
            Ok(linked) => linked,
            Err(_) => link.await,
        }
        .unwrap();

        assert!(linked["context_warning"].is_null());
        for file in ["AGENTS.md", "CLAUDE.md"] {
            let context = std::fs::read_to_string(root.join(file)).unwrap();
            assert!(context.contains("API-42 Repair the API"), "{file} lost the ticket after linking");
        }
    }

    #[tokio::test]
    async fn two_concurrent_links_for_one_ticket_cannot_claim_it_twice() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        let started = Arc::new(tokio::sync::Barrier::new(2));
        f.serve(Router::new().route(
            "/rest/api/3/issue/{key}",
            get(move || {
                let started = started.clone();
                async move {
                    started.wait().await;
                    jira_answer()
                }
            }),
        ));

        let (a, b) = tokio::join!(f.link("first", "old-42"), f.link("second", "API-42"));

        assert_ne!(a.is_ok(), b.is_ok(), "one link must win");
        let cfg = f.state().config.read();
        assert_eq!(cfg.tasks.iter().filter(|t| t.issue_key.as_deref() == Some("API-42")).count(), 1);
        assert!(a.err().or_else(|| b.err()).unwrap().to_string().contains("already linked to"));
    }

    #[tokio::test]
    async fn a_failed_lookup_does_not_save_a_link_or_create_context() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve(
            Router::new().route("/rest/api/3/issue/{key}", get(|| async { (StatusCode::NOT_FOUND, "No such ticket") })),
        );

        let err = f.link("first", "API-99").await.unwrap_err();

        assert!(err.to_string().contains("No such ticket"), "{err}");
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
        assert!(!f.task_root("first").join("TICKET.md").exists());
    }

    #[tokio::test]
    async fn linking_refuses_a_connection_changed_during_the_lookup() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        let app = f.handle();
        f.serve(Router::new().route(
            "/rest/api/3/issue/{key}",
            get(move || {
                let app = app.clone();
                async move {
                    app.state::<AppState>()
                        .config
                        .update(|c| c.jira.as_mut().unwrap().email = "another@example.test".into())
                        .unwrap();
                    jira_answer()
                }
            }),
        ));

        let err = f.link("first", "API-42").await.unwrap_err();

        assert!(err.to_string().contains("connection changed"), "{err}");
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
    }

    #[tokio::test]
    async fn a_context_write_failure_returns_the_link_that_was_saved() {
        let mut f = Fixture::new(TrackerKind::Jira).await;
        f.serve_jira();
        std::fs::create_dir(f.task_root("first").join("TICKET.md")).unwrap();
        let (send, mut received) = tokio::sync::mpsc::unbounded_channel();
        tauri::Listener::listen(&f.app, "task:ticket-linked", move |event| {
            let payload: Value = serde_json::from_str(event.payload()).unwrap();
            send.send(payload).unwrap();
        });

        let result = f.link("first", "API-42").await.unwrap();

        assert_eq!(result["task"]["issue_key"], "API-42");
        assert!(result["context_warning"].as_str().is_some_and(|w| w.contains("context")));
        assert_eq!(
            received.try_recv().expect("the window is notified even when context fails"),
            json!({
                "task_id": "first",
                "key": "API-42",
                "url": result["ticket"]["url"],
                "status": "To Do",
                "status_category": "new",
                "context_warning": result["context_warning"],
            })
        );
        let path = f.state().config.folder().join("config.json");
        let on_disk: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(on_disk["tasks"][0]["issue_key"], "API-42");
    }

    #[tokio::test]
    async fn linking_a_github_issue_outside_the_workspace_sends_no_request() {
        let mut f = Fixture::new(TrackerKind::Github).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        f.serve(Router::new().fallback(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                github_answer()
            }
        }));

        let err = f.link("first", "acme/private#42").await.unwrap_err();

        assert!(err.to_string().contains("not in one of the workspace's repositories"), "{err}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
    }

    #[tokio::test]
    async fn linking_rechecks_github_workspace_access_after_the_lookup() {
        let mut f = Fixture::new(TrackerKind::Github).await;
        let app = f.handle();
        f.serve(Router::new().route(
            "/graphql",
            post(move || {
                let app = app.clone();
                async move {
                    app.state::<AppState>().config.update(|c| c.projects[0].remote = None).unwrap();
                    github_answer()
                }
            }),
        ));

        let err = f.link("first", "acme/api#42").await.unwrap_err();

        assert!(err.to_string().contains("not in one of the workspace's repositories"), "{err}");
        assert!(f.state().config.task("first").unwrap().issue_key.is_none());
    }
}
