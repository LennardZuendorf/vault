//! The Henchman's tools (HEN-4): how every task stands, sorted as the lair
//! sorts it (HEN-5); what any agent or chat last said; a message to one;
//! and a new chat. Listed for, and answered to, the Henchman alone.

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::acp::Body;
use crate::config::Task;
use crate::error::{Error, Result};
use crate::mcp::{arg, required, str_prop, tool};
use crate::pty::{Activity, PaneInfo, PaneKind};

use super::github::CheckoutPr;
use super::henchman::{is_caller, is_henchman};
use super::panes::{hand_over, open_chat, typeable, Resume, CHAT_TASK_ID};
use super::AppState;

/// Where a message too long to type is left for a terminal agent.
pub(crate) const MESSAGE_FILE: &str = "HENCHMAN_MESSAGE.md";

/// How much of what a chat last said the overview carries.
const SAID: usize = 300;
/// How much of a turn `pane_read` gives back.
const TURN: usize = 6_000;

/// The Henchman's tools, for the Henchman's pane; nothing for another.
pub fn henchman_tools(app: &AppHandle, caller: Option<&str>) -> Vec<Value> {
    if !is_caller(&app.state::<AppState>(), caller) {
        return Vec::new();
    }
    listed()
}

fn listed() -> Vec<Value> {
    let pane = || str_prop("The pane's id, from work_overview");
    vec![
        tool(
            "work_overview",
            "How all of Layer stands, in one call. Every task in the lair's groups: needs_you \
             (ranked: an agent asks, is out of budget, or finished; changes asked for or checks \
             failed while no agent works), to_finish (every PR merged), moving (an agent works, \
             or a PR is open), not_started and quiet. Each with why, its ticket, agents and their \
             states, and pull requests (verdict, failed checks, replies owed). Then the open \
             chats, each with its state and what it last said; then the processes the panes \
             started that still run, with their ports.",
            json!({}),
            vec![],
        ),
        tool(
            "pane_read",
            "What one agent or chat last said: over ACP its last turn, whether it is still \
             busy, and any question it asks; in a terminal its recent output, only if the user \
             lets agents read terminal output.",
            json!({
                "pane_id": pane(),
                "lines": { "type": "integer", "description": "A terminal's lines from the end (default 80)" }
            }),
            vec!["pane_id"],
        ),
        tool(
            "pane_tell",
            "Send one message to a running agent or chat: typed into its terminal and \
             submitted, or sent as a prompt over ACP. It is marked as from you. Pass on the \
             user's words as they gave them.",
            json!({ "pane_id": pane(), "text": str_prop("The message") }),
            vec!["pane_id", "text"],
        ),
        tool(
            "chat_start",
            "Open a new chat in Layer's Chat tab, over ACP, with its first message. Only on the \
             user's word.",
            json!({
                "text": str_prop("Its first message"),
                "agent_id": str_prop("The agent CLI, e.g. claude; by default yours"),
            }),
            vec!["text"],
        ),
    ]
}

pub fn henchman_owns(name: &str) -> bool {
    matches!(name, "work_overview" | "pane_read" | "pane_tell" | "chat_start")
}

pub async fn henchman_call(app: &AppHandle, name: &str, args: Value, caller: Option<&str>) -> Result<Value> {
    let state = app.state::<AppState>();
    if !is_caller(&state, caller) {
        return Err(Error::Other(format!("{name} is the Henchman's; this pane is not the Henchman")));
    }
    match name {
        "work_overview" => overview(app).await,
        "pane_read" => {
            let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(80) as usize;
            read(&state, required(&args, "pane_id")?, lines)
        }
        "pane_tell" => tell(&state, required(&args, "pane_id")?, required(&args, "text")?),
        "chat_start" => {
            let text = required(&args, "text")?.to_string();
            let own = caller.and_then(|c| state.ptys.info(c).ok()).and_then(|p| p.agent_id);
            let agent = arg(&args, "agent_id").map(str::to_string).or(own).unwrap_or_else(|| "claude".into());
            let handle = app.clone();
            let pane =
                super::blocking(app.clone(), move |s| open_chat(&handle, s, agent, Some(text), None, Resume::No, true))
                    .await?;
            Ok(json!({ "pane_id": pane.id, "title": pane.title }))
        }
        _ => Err(Error::NotFound(format!("tool {name}"))),
    }
}

/// The lair's groups (HEN-5), in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Group {
    NeedsYou,
    ToFinish,
    Moving,
    NotStarted,
    Quiet,
}

/// What the rule needs to know of a task.
#[derive(Debug, Default)]
struct Facts {
    asking: bool,
    out_of_budget: bool,
    finished: bool,
    working: bool,
    changes_requested: bool,
    checks_failed: bool,
    /// Some PR open or merged, and every one of them merged.
    merged: bool,
    open: usize,
    approved: bool,
    agents: bool,
    prs: bool,
    changed: u32,
}

fn failed(conclusion: Option<&str>) -> bool {
    matches!(conclusion, Some("failure" | "timed_out" | "action_required" | "startup_failure"))
}

fn is_open(r: &CheckoutPr) -> bool {
    r.pr.as_ref().is_some_and(|p| p.state == "open")
}

fn facts(agents: &[PaneInfo], rows: &[CheckoutPr], changed: u32) -> Facts {
    let live: Vec<&PaneInfo> = agents.iter().filter(|p| p.kind == PaneKind::Agent && p.running).collect();
    let budget = |p: &&PaneInfo| p.notice.as_deref() == Some("usage_limit");
    let open: Vec<&CheckoutPr> = rows.iter().filter(|r| is_open(r)).collect();
    let landed: Vec<&CheckoutPr> =
        rows.iter().filter(|r| r.pr.as_ref().is_some_and(|p| p.state == "open" || p.merged)).collect();
    Facts {
        asking: live.iter().any(|p| p.activity == Activity::Asking && !budget(p)),
        out_of_budget: live.iter().any(budget),
        finished: live.iter().any(|p| p.activity == Activity::Done),
        working: live.iter().any(|p| p.activity == Activity::Working),
        changes_requested: open.iter().any(|r| r.verdict == "changes_requested"),
        checks_failed: open.iter().any(|r| r.checks.iter().any(|c| failed(c.conclusion.as_deref()))),
        merged: !landed.is_empty() && landed.iter().all(|r| r.pr.as_ref().is_some_and(|p| p.merged)),
        open: open.len(),
        approved: !open.is_empty() && open.iter().all(|r| r.verdict == "approved"),
        agents: !live.is_empty(),
        prs: rows.iter().any(|r| r.pr.is_some()),
        changed: changed.max(rows.iter().map(|r| r.changed as u32).max().unwrap_or(0)),
    }
}

/// The lair's rule (HEN-5): the group, a rank within Needs you, and why.
/// `lib/lair.ts` is the same rule for the window.
fn sort(f: &Facts) -> (Group, u8, &'static str) {
    if f.asking {
        return (Group::NeedsYou, 0, "its agent asks");
    }
    if f.out_of_budget {
        return (Group::NeedsYou, 1, "its agent is out of budget");
    }
    if f.finished {
        return (Group::NeedsYou, 2, "its agent finished: your turn");
    }
    if !f.working && f.changes_requested {
        return (Group::NeedsYou, 3, "changes asked for");
    }
    if !f.working && f.checks_failed {
        return (Group::NeedsYou, 4, "checks failed");
    }
    if f.merged {
        return (Group::ToFinish, 0, "merged");
    }
    if f.working {
        return (Group::Moving, 0, "its agent is working");
    }
    if f.open > 0 {
        return (Group::Moving, 0, if f.approved { "approved" } else { "in review" });
    }
    if !f.agents && !f.prs && f.changed == 0 {
        return (Group::NotStarted, 0, "not started");
    }
    (Group::Quiet, 0, if f.agents { "its agent is idle" } else { "nothing running" })
}

fn pane_name(p: &PaneInfo) -> String {
    p.topic.clone().unwrap_or_else(|| p.title.clone())
}

fn pane_state(p: &PaneInfo) -> &'static str {
    if !p.running {
        return "exited";
    }
    if p.notice.as_deref() == Some("usage_limit") {
        return "out of budget";
    }
    match (p.kind, p.activity) {
        (PaneKind::Shell, _) => "shell",
        (_, Activity::Working) => "working",
        (_, Activity::Asking) => "asks",
        (_, Activity::Done) => "finished",
        (_, Activity::Idle) => "idle",
    }
}

fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// What a conversation's last turn said, and any question still open.
fn last_turn<'a>(bodies: impl IntoIterator<Item = &'a Body>) -> (String, Option<Value>) {
    let mut said: Vec<&str> = Vec::new();
    let mut asks = None;
    for body in bodies {
        match body {
            Body::User { dropped: false, .. } => {
                said.clear();
                asks = None;
            }
            Body::Agent { text } if !text.trim().is_empty() => said.push(text.trim()),
            Body::Question { title, options, answer: None } => {
                asks = Some(json!({ "title": title, "options": options.iter().map(|o| &o.name).collect::<Vec<_>>() }));
            }
            Body::Question { answer: Some(_), .. } => asks = None,
            _ => {}
        }
    }
    (said.join("\n\n"), asks)
}

/// Uncommitted, staged and unpushed work and files changed, as the status
/// cache last had them: what tells a task that was started from one that
/// was not.
fn changed(state: &AppState, task_id: &str) -> u32 {
    let ids: Vec<String> = state.config.checkouts_of(task_id).into_iter().map(|c| c.id).collect();
    let cache = state.status_cache.lock();
    ids.iter()
        .filter_map(|id| cache.get(id))
        .map(|c| c.changed + c.status.as_ref().map(|s| s.ahead + s.staged + s.unstaged + s.untracked).unwrap_or(0))
        .sum()
}

/// Each task's PR rows: as last read, and read now for a task never read.
async fn prs_of(state: &AppState, tasks: &[Task]) -> std::collections::HashMap<String, Vec<CheckoutPr>> {
    let missing: Vec<String> = {
        let seen = state.prs_seen.lock();
        tasks.iter().filter(|t| t.review.is_none() && !seen.contains_key(&t.id)).map(|t| t.id.clone()).collect()
    };
    if !missing.is_empty() {
        if let Ok((client, _)) = super::github::github_client(state) {
            use futures_util::stream::{self, StreamExt};
            let client = &client;
            stream::iter(missing)
                .map(|id| async move { super::github::task_prs(state, client, &id).await })
                .buffered(4)
                .collect::<Vec<_>>()
                .await;
        }
    }
    state.prs_seen.lock().clone()
}

async fn overview(app: &AppHandle) -> Result<Value> {
    let state = app.state::<AppState>();
    let tasks = state.config.read().tasks;
    let prs = prs_of(&state, &tasks).await;
    let panes = state.ptys.list(None);
    let processes = super::blocking(app.clone(), super::processes::list).await.unwrap_or_default();

    let mut rows: Vec<(Group, u8, Value)> = tasks
        .iter()
        .map(|task| {
            let agents: Vec<PaneInfo> = panes.iter().filter(|p| p.task_id == task.id).cloned().collect();
            let pr_rows = prs.get(&task.id).map(Vec::as_slice).unwrap_or_default();
            let (group, rank, why) = sort(&facts(&agents, pr_rows, changed(&state, &task.id)));
            let row = json!({
                "task_id": task.id,
                "key": task.issue_key,
                "name": task.name,
                "epic": task.epic,
                "group": group,
                "why": why,
                "agents": agents.iter().filter(|p| p.kind == PaneKind::Agent).map(|p| json!({
                    "pane_id": p.id, "name": pane_name(p), "state": pane_state(p),
                })).collect::<Vec<_>>(),
                "prs": pr_rows.iter().filter_map(|r| r.pr.as_ref().map(|pr| json!({
                    "repo": r.repo,
                    "number": pr.number,
                    "url": pr.url,
                    "state": if pr.merged { "merged" } else { pr.state.as_str() },
                    "draft": pr.draft,
                    "verdict": r.verdict,
                    "checks_failed": r.checks.iter().filter(|c| failed(c.conclusion.as_deref())).map(|c| &c.name).collect::<Vec<_>>(),
                    "to_reply": r.to_reply,
                }))).collect::<Vec<_>>(),
            });
            (group, rank, row)
        })
        .collect();
    rows.sort_by_key(|(group, rank, _)| (*group, *rank));
    let count = |g: Group| rows.iter().filter(|(group, _, _)| *group == g).count();

    let chats: Vec<Value> = panes
        .iter()
        .filter(|p| p.task_id == CHAT_TASK_ID && p.kind == PaneKind::Agent && !is_henchman(&state, p))
        .map(|p| {
            let said = p
                .acp
                .then(|| state.ptys.acp_read(&p.id, None).ok())
                .flatten()
                .map(|v| cut(&last_turn(v.changes.entries.iter().map(|e| &e.body)).0, SAID));
            json!({ "pane_id": p.id, "name": pane_name(p), "state": pane_state(p), "said": said })
        })
        .collect();
    let chats_asking = panes
        .iter()
        .filter(|p| p.task_id == CHAT_TASK_ID && p.running && p.activity == Activity::Asking && !is_henchman(&state, p))
        .count();

    Ok(json!({
        "counts": {
            "open": rows.len(),
            "needs_you": count(Group::NeedsYou) + chats_asking,
            "to_finish": count(Group::ToFinish),
            "moving": count(Group::Moving),
            "not_started": count(Group::NotStarted),
            "quiet": count(Group::Quiet),
        },
        "tasks": rows.into_iter().map(|(_, _, row)| row).collect::<Vec<_>>(),
        "chats": chats,
        "processes": processes,
        "github": super::github::github_client(&state).is_ok(),
    }))
}

fn read(state: &AppState, pane_id: &str, lines: usize) -> Result<Value> {
    let info = state.ptys.info(pane_id)?;
    let head = json!({ "pane_id": info.id, "name": pane_name(&info), "state": pane_state(&info) });
    if info.acp {
        let view = state.ptys.acp_read(pane_id, None)?;
        let (said, asks) = last_turn(view.changes.entries.iter().map(|e| &e.body));
        return Ok(json!({ "pane": head, "busy": view.busy, "said": cut(&said, TURN), "asks": asks }));
    }
    let output = super::pane_output(state, pane_id, lines)?;
    Ok(json!({ "pane": head, "output": output }))
}

fn tell(state: &AppState, pane_id: &str, text: &str) -> Result<Value> {
    let info = state.ptys.info(pane_id)?;
    if is_henchman(state, &info) {
        return Err(Error::Other("that pane is you".into()));
    }
    if info.kind != PaneKind::Agent {
        return Err(Error::Other("that is a shell: pane_tell is for agents and chats".into()));
    }
    if !info.running {
        return Err(Error::Other(format!("{} has exited", pane_name(&info))));
    }
    let text = format!("[villain-layer · from the Henchman] {text}");
    if info.task_id == CHAT_TASK_ID {
        let typed = if info.acp { text } else { typeable(Some(Path::new(&info.cwd)), MESSAGE_FILE, &text)? };
        state.ptys.submit(pane_id, &typed)?;
    } else {
        let task = state.config.task(&info.task_id)?;
        hand_over(state, &task, pane_id, MESSAGE_FILE, &text)?;
    }
    Ok(json!({ "told": pane_name(&info) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_that_asks_outranks_everything_else_on_the_task() {
        let f = Facts { asking: true, finished: true, changes_requested: true, merged: true, ..Facts::default() };
        assert_eq!(sort(&f), (Group::NeedsYou, 0, "its agent asks"));
    }

    #[test]
    fn needs_you_is_ranked_asks_budget_finished_changes_checks() {
        let ranks: Vec<u8> = [
            Facts { out_of_budget: true, ..Facts::default() },
            Facts { finished: true, ..Facts::default() },
            Facts { changes_requested: true, ..Facts::default() },
            Facts { checks_failed: true, ..Facts::default() },
        ]
        .iter()
        .map(|f| sort(f).1)
        .collect();
        assert_eq!(ranks, vec![1, 2, 3, 4]);
    }

    #[test]
    fn changes_or_red_checks_wait_while_its_agent_works_on_them() {
        let f = Facts { working: true, changes_requested: true, checks_failed: true, ..Facts::default() };
        assert_eq!(sort(&f), (Group::Moving, 0, "its agent is working"));
    }

    #[test]
    fn every_pr_merged_is_to_finish() {
        let f = Facts { merged: true, prs: true, ..Facts::default() };
        assert_eq!(sort(&f).0, Group::ToFinish);
    }

    #[test]
    fn an_open_pr_is_moving_and_says_whether_it_is_approved() {
        assert_eq!(sort(&Facts { open: 1, prs: true, ..Facts::default() }).2, "in review");
        assert_eq!(sort(&Facts { open: 1, prs: true, approved: true, ..Facts::default() }).2, "approved");
    }

    #[test]
    fn nothing_at_all_is_not_started_and_anything_left_is_quiet() {
        assert_eq!(sort(&Facts::default()).0, Group::NotStarted);
        assert_eq!(sort(&Facts { changed: 3, ..Facts::default() }).0, Group::Quiet);
        assert_eq!(sort(&Facts { agents: true, ..Facts::default() }), (Group::Quiet, 0, "its agent is idle"));
    }

    #[test]
    fn a_turn_is_what_the_agent_said_since_the_last_prompt_with_its_open_question() {
        let bodies = vec![
            Body::Agent { text: "old".into() },
            Body::User { text: "go".into(), queued: false, dropped: false },
            Body::Agent { text: "first".into() },
            Body::Agent { text: "second".into() },
            Body::Question { title: "Write a.rs?".into(), options: Vec::new(), answer: None },
        ];
        let (said, asks) = last_turn(&bodies);
        assert_eq!(said, "first\n\nsecond");
        assert_eq!(asks.and_then(|a| a.get("title").cloned()), Some(json!("Write a.rs?")));
    }
}
