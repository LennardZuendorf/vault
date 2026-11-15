//! Work the Henchman starts by itself (HEN-11): a message to a task's agent,
//! which wakes it if it is idle, picks it up on its conversation if it
//! exited, or starts one if the task has none. Only for a task already
//! started (an agent or an open pull request), or one the user asked it to
//! start; what the agent then does lands on the task's own branch. Called
//! from the Henchman's tools, off the main thread.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::agents;
use crate::config::Task;
use crate::error::{Error, Result};
use crate::pty::{Activity, PaneInfo, PaneKind};

use super::henchman_tools::{pane_name, restart_exited, tell};
use super::panes::{start_agent, Resume};
use super::AppState;

/// Agents the Henchman may start or pick up within `HOUR`. A ceiling that
/// stops a runaway (each change on a pull request starting one more agent),
/// not a pace: waking an agent that runs is not counted.
const STARTS: usize = 8;
const HOUR: Duration = Duration::from_secs(60 * 60);

/// When the Henchman started or picked up an agent.
static STARTED: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// What becomes of the message, from the task's agents.
#[derive(Debug, PartialEq)]
enum Plan {
    /// To this idle agent.
    Tell(usize),
    /// Refused: this one is in the middle of a turn.
    Busy(usize),
    /// Refused: this one asks the user something.
    Asks(usize),
    /// This one exited: picked up on its conversation, then told.
    PickUp(usize),
    /// None to tell: a new one, with the message as its first prompt.
    Start,
}

/// One running agent decides it, the newest first; else the newest that
/// exited, if it can be picked up over ACP, where a prompt sent while its
/// conversation replays waits for it. A terminal agent that exited is not:
/// its CLI takes no prompt on a resumed conversation.
fn plan(agents: &[PaneInfo]) -> Plan {
    let mut order: Vec<usize> = (0..agents.len()).filter(|&i| agents[i].kind == PaneKind::Agent).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(agents[i].started_at));
    if let Some(&i) = order.iter().find(|&&i| agents[i].running && agents[i].activity == Activity::Working) {
        return Plan::Busy(i);
    }
    if let Some(&i) = order.iter().find(|&&i| agents[i].running && agents[i].activity == Activity::Asking) {
        return Plan::Asks(i);
    }
    if let Some(&i) = order.iter().find(|&&i| agents[i].running) {
        return Plan::Tell(i);
    }
    match order.first() {
        Some(&i) if agents[i].acp => Plan::PickUp(i),
        _ => Plan::Start,
    }
}

fn started_lately(list: &[Instant], now: Instant) -> usize {
    list.iter().filter(|at| now.duration_since(**at) < HOUR).count()
}

/// The task by its id or its ticket key.
fn find(state: &AppState, task: &str) -> Result<Task> {
    state
        .config
        .with(|c| {
            c.tasks
                .iter()
                .find(|t| t.id == task || t.issue_key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(task)))
                .cloned()
        })
        .ok_or_else(|| {
            Error::NotFound(format!(
                "no task {task}: use a task_id or key from work_overview; for a ticket with no task, start_work \
                 makes one, on the user's word"
            ))
        })
}

/// Give a task's agent `text` (HEN-11). `agent` is the CLI for a new one;
/// by default the task's last, else `own`, the Henchman's. `asked`: the user
/// asked the Henchman to start work on this task.
pub(crate) fn task_work(
    app: &AppHandle,
    state: &AppState,
    task: &str,
    text: &str,
    agent: Option<String>,
    own: Option<String>,
    asked: bool,
) -> Result<Value> {
    let task = find(state, task)?;
    let key = task.issue_key.clone().unwrap_or_else(|| task.name.clone());
    if state.config.with(|c| c.henchman.left_alone.contains(&task.id)) {
        return Err(Error::Other(format!("the user left {key} alone: tell them instead")));
    }
    let agents: Vec<PaneInfo> =
        state.ptys.list(Some(&task.id)).into_iter().filter(|p| p.kind == PaneKind::Agent).collect();
    let open_pr = state
        .prs_seen
        .lock()
        .get(&task.id)
        .is_some_and(|rows| rows.iter().any(|r| r.pr.as_ref().is_some_and(|p| p.state == "open")));
    // Starting on a ticket is the user's: the work is theirs to begin. Their
    // word is enough: told "start ACME-12", it was refused all the same.
    if agents.is_empty() && !open_pr && !asked {
        return Err(Error::Other(format!(
            "{key} isn't started: start it only when the user asks you to, with asked: true"
        )));
    }

    let plan = plan(&agents);
    if matches!(plan, Plan::PickUp(_) | Plan::Start) && started_lately(&STARTED.lock(), Instant::now()) >= STARTS {
        return Err(Error::Other(format!(
            "you started {STARTS} agents in the last hour: ask the user before you start more"
        )));
    }
    match plan {
        Plan::Busy(i) => Err(Error::Other(format!(
            "{}'s agent {} is in the middle of a turn: you hear when it finishes",
            key,
            pane_name(&agents[i])
        ))),
        Plan::Asks(i) => Err(Error::Other(format!(
            "{}'s agent {} asks the user something, which only they answer: tell them",
            key,
            pane_name(&agents[i])
        ))),
        Plan::Tell(i) => {
            tell(state, &agents[i].id, text)?;
            Ok(json!({ "told": pane_name(&agents[i]), "pane_id": agents[i].id }))
        }
        Plan::PickUp(i) => {
            // As pane_restart: one that exits again is not picked up twice.
            let pane = restart_exited(app, state, &agents[i].id)?;
            STARTED.lock().push(Instant::now());
            tell(state, &pane.id, text)?;
            Ok(json!({ "picked_up": pane_name(&pane), "pane_id": pane.id }))
        }
        Plan::Start => {
            let newest = agents.iter().max_by_key(|p| p.started_at).and_then(|p| p.agent_id.clone());
            let agent_id = agent.or(newest).or(own).unwrap_or_else(|| "claude".into());
            let def = agents::find(&agent_id).ok_or_else(|| Error::NotFound(format!("agent {agent_id}")))?;
            // Over ACP where the CLI speaks it: the Henchman reads its turns
            // and the errors they end on, which a terminal keeps from it.
            let acp = def.acp.is_some();
            let first = format!("[villain-layer · from the Henchman] {text}");
            let pane =
                start_agent(app, state, task.id.clone(), agent_id, None, Some(first), Resume::No, None, None, acp)?;
            STARTED.lock().push(Instant::now());
            Ok(json!({ "started": pane_name(&pane), "pane_id": pane.id }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, running: bool, activity: Activity, age: i64) -> PaneInfo {
        let at = chrono::Utc::now() - chrono::Duration::minutes(age);
        PaneInfo {
            id: id.into(),
            task_id: "t1".into(),
            checkout_id: None,
            kind: PaneKind::Agent,
            title: "Claude Code".into(),
            agent_id: Some("claude".into()),
            cwd: "/tmp".into(),
            running,
            exit_code: None,
            started_at: at,
            last_output_at: at,
            notice: None,
            activity,
            activity_since: at,
            topic: None,
            acp: true,
            loop_said: None,
        }
    }

    #[test]
    fn a_running_agent_is_told_and_a_busy_or_asking_one_refuses_it() {
        assert_eq!(plan(&[agent("a", true, Activity::Done, 1)]), Plan::Tell(0));
        let two = [agent("idle", true, Activity::Idle, 1), agent("busy", true, Activity::Working, 5)];
        assert_eq!(plan(&two), Plan::Busy(1), "one working on the task is enough");
        assert_eq!(plan(&[agent("q", true, Activity::Asking, 1)]), Plan::Asks(0));
    }

    #[test]
    fn of_several_idle_agents_the_newest_is_told() {
        let two = [agent("old", true, Activity::Idle, 30), agent("new", true, Activity::Done, 2)];
        assert_eq!(plan(&two), Plan::Tell(1));
    }

    #[test]
    fn an_exited_conversation_is_picked_up_and_an_exited_terminal_or_none_starts_a_new_one() {
        assert_eq!(plan(&[agent("gone", false, Activity::Idle, 3)]), Plan::PickUp(0));
        let mut terminal = agent("term", false, Activity::Idle, 3);
        terminal.acp = false;
        assert_eq!(plan(&[terminal]), Plan::Start);
        assert_eq!(plan(&[]), Plan::Start);
    }

    #[test]
    fn a_shell_is_never_given_the_work() {
        let mut shell = agent("sh", true, Activity::Idle, 1);
        shell.kind = PaneKind::Shell;
        assert_eq!(plan(&[shell]), Plan::Start);
    }

    #[test]
    fn only_the_last_hours_starts_count_toward_the_ceiling() {
        let now = Instant::now();
        let Some(long_ago) = now.checked_sub(HOUR + Duration::from_secs(1)) else {
            return;
        };
        assert_eq!(started_lately(&[now, now, long_ago], now), 2);
    }
}
