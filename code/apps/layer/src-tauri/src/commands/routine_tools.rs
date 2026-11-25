//! Routines' tools for chats (ROUT-11): a chat's agent lists, makes,
//! changes, runs and deletes routines as the Routines view does, through
//! the same functions, so the view shows each change as it happens. Only a
//! chat is offered them: an agent at work on a task has no reason to set
//! up what runs on this Mac every morning.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::mcp::{bool_prop, confirm_prop, required, str_prop, tool};
use crate::routines::{self, RoutineView, Run};

use super::panes::CHAT_TASK_ID;
use super::routines::{create, delete, start, update, RoutinePatch};
use super::AppState;

/// How many of a routine's runs `list_routines` tells of, newest first.
const RUNS: usize = 5;

fn is_chat(state: &AppState, caller: Option<&str>) -> bool {
    caller.and_then(|id| state.ptys.info(id).ok()).is_some_and(|p| p.task_id == CHAT_TASK_ID)
}

/// The routines' tools, for a chat; none for any other caller.
pub fn routine_tools(app: &AppHandle, caller: Option<&str>) -> Vec<Value> {
    if !is_chat(&app.state::<AppState>(), caller) {
        return Vec::new();
    }
    routine_listed()
}

/// Every one of them, whoever asks: `mcp.rs`'s tests check the gate on it.
pub fn routine_listed() -> Vec<Value> {
    let fields = || {
        json!({
            "name": str_prop("What it is called in the Routines view"),
            "prompt": str_prop("What it asks: the first message of each run's chat"),
            "every": { "type": "string", "enum": ["hourly", "daily", "weekdays", "weekly"],
                       "description": "How often: hourly at a minute, daily, Monday to Friday, or weekly on a day" },
            "at": str_prop("The time on this Mac's clock, HH:MM, e.g. 08:00; an hourly routine reads only the minutes"),
            "weekday": { "type": "integer", "minimum": 0, "maximum": 6,
                         "description": "For weekly: 0 is Monday, 6 Sunday" },
            "agent_id": str_prop("An agent that runs as a conversation, e.g. claude; by default the Henchman's"),
            "model": str_prop("Its model; empty lets the agent decide"),
            "effort": str_prop("low, medium or high; empty lets the agent decide"),
            "on": bool_prop("Whether it runs on its schedule; on by default")
        })
    };
    let mut change = fields();
    change["id"] = str_prop("The routine's id, from list_routines");
    vec![
        tool(
            "list_routines",
            "The routines on this Mac (prompts Layer runs on a schedule, each run a new chat): \
             what each asks, when, with which agent, whether it is on, when it runs next, \
             and its newest runs with what they said.",
            json!({}),
            vec![],
        ),
        tool(
            "create_routine",
            "Make a routine: a prompt Layer asks on a schedule, each time as a new chat. \
             It waits for its first time; run_routine runs it now. Returns it.",
            fields(),
            vec!["name", "prompt"],
        ),
        tool(
            "update_routine",
            "Change a routine. A field left out stays. Another time, or turning it on, \
             waits for the next time. Returns it.",
            change,
            vec!["id"],
        ),
        tool(
            "run_routine",
            "Run a routine now, whatever its schedule, as a new chat. Refused while its \
             last run still works or asks, or while it asks nothing.",
            json!({ "id": str_prop("The routine's id, from list_routines") }),
            vec!["id"],
        ),
        tool(
            "delete_routine",
            "Delete a routine and all its runs, stopping one still at work.",
            json!({ "id": str_prop("The routine's id, from list_routines"), "confirm": confirm_prop() }),
            vec!["id"],
        ),
    ]
}

pub fn routine_owns(name: &str) -> bool {
    matches!(name, "list_routines" | "create_routine" | "update_routine" | "run_routine" | "delete_routine")
}

/// The confirm gate is `mcp.rs`'s: `delete_routine` is in its `DANGER` list.
pub async fn routine_call(app: &AppHandle, name: &str, args: Value, caller: Option<&str>) -> Result<Value> {
    if !is_chat(&app.state::<AppState>(), caller) {
        return Err(Error::Other(format!("{name} is for chats; this pane is not a chat")));
    }
    let handle = app.clone();
    let name = name.to_string();
    super::blocking(app.clone(), move |state| match name.as_str() {
        "list_routines" => {
            let view = routines::read(&handle, state.config.folder(), |all| all.view());
            Ok(json!({ "routines": view.routines.iter().map(|r| shown(r, &view.runs)).collect::<Vec<_>>() }))
        }
        "create_routine" => {
            let id = create(&handle, state, patch(&args)?)?;
            one(&handle, state, &id)
        }
        "update_routine" => {
            let id = required(&args, "id")?.to_string();
            update(&handle, state, &id, patch(&args)?)?;
            one(&handle, state, &id)
        }
        "run_routine" => {
            let run = start(&handle, state, required(&args, "id")?, None, false)?;
            Ok(run_shown(&run))
        }
        "delete_routine" => {
            delete(&handle, state, required(&args, "id")?)?;
            Ok(json!({ "deleted": true }))
        }
        other => Err(Error::Other(format!("unknown tool {other}"))),
    })
    .await
}

/// The fields of a routine the call sets; `id` and `confirm` are not one.
fn patch(args: &Value) -> Result<RoutinePatch> {
    serde_json::from_value(args.clone()).map_err(|e| Error::Other(format!("Those fields are not a routine's: {e}")))
}

fn one(app: &AppHandle, state: &AppState, id: &str) -> Result<Value> {
    let view = routines::read(app, state.config.folder(), |all| all.view());
    let r =
        view.routines.iter().find(|r| r.routine.id == id).ok_or_else(|| Error::NotFound(format!("routine {id}")))?;
    Ok(shown(r, &view.runs))
}

fn shown(r: &RoutineView, runs: &[Run]) -> Value {
    let mut v = serde_json::to_value(r).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("last_due");
        o.insert(
            "runs".into(),
            runs.iter().filter(|x| x.routine_id == r.routine.id).take(RUNS).map(run_shown).collect(),
        );
    }
    v
}

fn run_shown(run: &Run) -> Value {
    json!({
        "id": run.id,
        "due": run.due,
        "started": run.started,
        "caught_up": run.caught_up,
        "state": run.state,
        "said": run.said,
        "ended": run.ended,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routines::Every;

    #[test]
    fn a_tools_arguments_become_the_routines_fields_and_the_rest_is_left_out() {
        let p =
            patch(&json!({ "id": "r1", "confirm": true, "name": "Brief", "every": "weekdays", "weekday": 2 })).unwrap();
        assert_eq!(p.name.as_deref(), Some("Brief"));
        assert_eq!(p.every, Some(Every::Weekdays));
        assert_eq!(p.weekday, Some(2));
        assert!(p.prompt.is_none() && p.at.is_none() && p.on.is_none());
    }

    #[test]
    fn a_schedule_the_routines_do_not_know_is_refused_with_why() {
        let e = patch(&json!({ "every": "monthly" })).unwrap_err().to_string();
        assert!(e.contains("monthly"), "{e}");
    }

    #[test]
    fn every_tool_listed_is_one_this_module_answers() {
        for name in ["list_routines", "create_routine", "update_routine", "run_routine", "delete_routine"] {
            assert!(routine_owns(name), "{name}");
        }
        assert!(!routine_owns("list_tasks"));
    }
}
