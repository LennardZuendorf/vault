//! `finish_task` for agents (TASK-18): what Finish task does in the app, for
//! a chat the user asks to put a landed task away. The finishing itself is
//! `tasks.rs`'s; this works out the done-category transition for the ticket.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::integrations::jira::Transition;
use crate::mcp::{bool_prop, required, str_prop, tool};

use super::ticket_flow::{jira_changed, JiraConnection};
use super::AppState;

#[derive(serde::Serialize)]
pub struct FinishTransitions {
    scope: String,
    transitions: Vec<Transition>,
}

/// List only transitions for the Jira site in the task's saved link.
#[tauri::command]
pub async fn finish_task_transitions(state: State<'_, AppState>, task_id: String) -> Result<Option<FinishTransitions>> {
    let task = state.config.task(&task_id)?;
    let jira = match JiraConnection::load(&state) {
        Ok(jira) => jira,
        Err(Error::NotConfigured("Jira")) => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(key) = jira.key(&task) else { return Ok(None) };
    jira.require_ticket(&state, &task)?;
    let transitions = done_transitions(&jira, key).await?;
    jira.require_ticket(&state, &task)?;
    Ok(Some(FinishTransitions { scope: jira.scope(&task)?, transitions }))
}

/// Finish locally, then apply the chosen Jira transition if its scope still matches.
#[tauri::command]
pub async fn finish_task(
    app: AppHandle,
    task_id: String,
    transition_id: Option<String>,
    transition_scope: Option<String>,
) -> Result<super::tasks::Finished> {
    let transition = if let Some(id) = transition_id {
        let state = app.state::<AppState>();
        let task = state.config.task(&task_id)?;
        let jira = JiraConnection::load(&state)?;
        jira.require_ticket(&state, &task)?;
        if transition_scope.as_deref() != Some(jira.scope(&task)?.as_str()) {
            return Err(jira_changed());
        }
        Some((jira, id))
    } else {
        None
    };
    super::tasks::finish_task_with(app, task_id, transition).await
}

pub fn finish_tools() -> Vec<Value> {
    vec![tool(
        "finish_task",
        "Finish a task whose work has landed, as Finish task in the app does: stop its \
         agents, remove its worktrees, delete each local branch its merged pull request \
         contains, and move its ticket into a done status. Refused while one of its pull \
         requests is still open. Without `transition`, the ticket goes to the status chosen \
         for merged work in its project, or the only done one; when that would be a guess \
         it is left, and the reply lists the ways to close it (use jira_transition). A \
         worktree with uncommitted changes keeps the task, and nothing after it is done. \
         Needs confirm: true after the user agrees; the user asking you to finish it is \
         that agreement.",
        json!({
            "task_id": str_prop("Task id from list_tasks"),
            "transition": str_prop(
                "The done-category transition, or the status it goes to, e.g. Done; \
                 only when the user named one or the reply asked for it"
            ),
            "confirm": bool_prop(
                "Set true only after the user agrees. Calls without it are refused — \
                 this tool stops agents and removes worktrees.",
            ),
        }),
        vec!["task_id"],
    )]
}

pub fn finish_owns(name: &str) -> bool {
    name == "finish_task"
}

/// The confirm gate is `mcp.rs`'s: `finish_task` is in its `DANGER` list.
/// Everything that can refuse is asked before anything is stopped: once the
/// task is gone there is nothing left to try again on.
pub async fn finish_call(app: &AppHandle, args: Value, caller: Option<&str>) -> Result<Value> {
    let state = app.state::<AppState>();
    let task = state.config.task(required(&args, "task_id")?)?;
    super::pr_task::not_a_review(&task, "finished by an agent")?;
    // Its own agents are stopped first, this one with them, and the reply
    // would never reach it.
    if let Some(pane) = caller.and_then(|p| state.ptys.info(p).ok()) {
        if pane.task_id == task.id {
            return Err(Error::Other(format!(
                "you are running in {}, and finishing it stops you before it is done. \
                 Ask the user to finish it from a chat, or with Finish task in the app.",
                task.name
            )));
        }
    }

    let wanted = args.get("transition").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    let jira = JiraConnection::load(&state).ok().filter(|jira| jira.key(&task).is_some());
    let ticket = match &jira {
        Some(jira) => {
            let key = jira.key(&task).ok_or_else(jira_changed)?;
            Some((key.to_owned(), closing(&state, jira, &task, key, wanted).await?))
        }
        _ if wanted.is_some() => {
            return Err(Error::Other(format!("{} has no Jira ticket to move; nothing was changed", task.name)));
        }
        _ => None,
    };
    let transition_id = ticket.as_ref().and_then(|(_, c)| c.chosen.as_ref()).map(|t| t.id.clone());

    let transition = jira.zip(transition_id);
    let finished = super::tasks::finish_task_with(app.clone(), task.id.clone(), transition).await?;
    let kept = finished.repos.iter().any(|r| !r.ok);
    let mut out = json!({
        "finished": !kept,
        "repos": finished.repos,
        "branches": finished.branches,
    });
    if kept {
        out["note"] = json!(format!(
            "{} was kept: git would not remove the worktrees listed. Nothing else was done; \
             the ticket was not moved.",
            task.name
        ));
    } else if let Some((key, closing)) = ticket {
        out["ticket"] = match (closing, finished.ticket_moved, finished.ticket_error) {
            (Closing { already: Some(status), .. }, _, _) => json!({ "key": key, "left": format!("already {status}") }),
            (Closing { chosen: Some(t), .. }, true, _) => json!({ "key": key, "moved_to": t.to_status }),
            (_, _, Some(e)) => json!({ "key": key, "error": e }),
            (Closing { options, .. }, _, _) => json!({
                "key": key,
                "left": "more than one done status, and none chosen for merged work in its project",
                "ways_to_close": options,
            }),
        };
    }
    Ok(out)
}

/// UI and MCP share this guard. Closed or missing PRs are not proof that the
/// task landed, and a non-primary open PR still blocks Finish.
pub(super) fn validate_finish(rows: &[super::pr_resolver::Resolved], recovering: bool) -> Result<()> {
    for row in rows {
        row.require_complete()?;
        if let Some(pr) = row.row.open() {
            return Err(Error::Other(format!(
                "{}'s pull request #{} is still open. Nothing was changed.",
                row.row.repo, pr.number
            )));
        }
        if row.row.detached_unlanded {
            return Err(Error::Other(format!(
                "{} has unlanded commits on a detached HEAD. Keep them on a branch before finishing.",
                row.row.repo
            )));
        }
    }
    if !rows.iter().any(|r| r.row.covered_merge)
        && !(recovering && rows.iter().all(|r| r.row.branch_covered && r.row.changed == 0))
    {
        return Err(Error::Other(
            "No merged pull request could be verified for this task. Nothing was changed.".into(),
        ));
    }
    Ok(())
}

struct Closing {
    /// The ticket is in a done status already: it is left there.
    already: Option<String>,
    chosen: Option<Transition>,
    options: Vec<String>,
}

/// Only into Jira's done category, by category and never by name (TASK-9).
/// Picked without being told only when it is known: the project's status for
/// merged work (TKT-8), or the only done one. Won't Do is done too.
async fn closing(
    state: &AppState,
    jira: &JiraConnection,
    task: &crate::config::Task,
    key: &str,
    wanted: Option<&str>,
) -> Result<Closing> {
    jira.require_ticket(state, task)?;
    let issue = jira.client.issue(key).await?;
    jira.require_ticket(state, task)?;
    if issue.status_category == "done" && wanted.is_none() {
        return Ok(Closing { already: Some(issue.status), chosen: None, options: Vec::new() });
    }
    let done = done_transitions(jira, key).await?;
    jira.require_ticket(state, task)?;
    let project = key.split('-').next().unwrap_or_default();
    let merged = jira.config.flow.get(project).and_then(|f| f.merged.as_ref()).map(|m| m.id.as_str());
    let options = done.iter().map(|t| format!("{} (to {})", t.name, t.to_status)).collect::<Vec<_>>();
    let chosen = pick(&done, wanted, merged).map_err(|w| {
        Error::Other(format!(
            "{key} has no done-category transition named {w}; it can close by: {}. \
             Nothing was changed.",
            if options.is_empty() { "none from where it is".to_string() } else { options.join(", ") }
        ))
    })?;
    Ok(Closing { already: None, chosen, options })
}

async fn done_transitions(jira: &JiraConnection, key: &str) -> Result<Vec<Transition>> {
    Ok(jira.client.transitions(key).await?.into_iter().filter(|t| t.to_category == "done").collect())
}

/// The one named, by transition or status, or else the one into the
/// project's merged status, or else the only one. `Err` gives back a name
/// that matched nothing.
fn pick<'a>(
    done: &[Transition],
    wanted: Option<&'a str>,
    merged: Option<&str>,
) -> std::result::Result<Option<Transition>, &'a str> {
    if let Some(w) = wanted {
        return done
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(w) || t.to_status.eq_ignore_ascii_case(w))
            .cloned()
            .map(Some)
            .ok_or(w);
    }
    Ok(done
        .iter()
        .find(|t| merged.is_some_and(|m| m == t.to_id))
        .or_else(|| (done.len() == 1).then(|| &done[0]))
        .cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(name: &str, status: &str, id: &str) -> Transition {
        Transition {
            id: format!("t{id}"),
            name: name.into(),
            to_status: status.into(),
            to_id: id.into(),
            to_category: "done".into(),
        }
    }

    #[test]
    fn a_ticket_closes_into_the_status_chosen_for_merged_work_and_otherwise_only_when_there_is_one_way() {
        let both = [to("Won't do", "Won't Do", "1"), to("Close", "Done", "2")];
        assert_eq!(pick(&both, None, Some("2")).unwrap().map(|t| t.id), Some("t2".into()));
        assert!(pick(&both, None, None).unwrap().is_none(), "first in the list once closed merged work as Won't Do");
        assert_eq!(pick(&both[1..], None, None).unwrap().map(|t| t.id), Some("t2".into()));
    }

    #[test]
    fn a_named_close_matches_the_transition_or_the_status_and_an_unknown_one_is_refused() {
        let done = [to("Close", "Done", "2")];
        assert_eq!(pick(&done, Some("done"), None).unwrap().map(|t| t.id), Some("t2".into()));
        assert_eq!(pick(&done, Some("CLOSE"), None).unwrap().map(|t| t.id), Some("t2".into()));
        assert_eq!(pick(&done, Some("In Review"), Some("2")).unwrap_err(), "In Review");
    }
}
