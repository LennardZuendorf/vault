//! A spec changed outside the Spec tab (§19): a copy in a repository that
//! is not what was approved (SPEC-22), put back; and a change an agent
//! proposed with `spec_propose` (SPEC-24), turned down or (in `spec.rs`)
//! approved. An agent that edited a spec file used to be believed: the tab
//! showed its text as approved, nothing after it went out of date, and it
//! was committed with the code.

use std::path::Path;

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::config::Task;
use crate::error::{Error, Result};
use crate::git;
use crate::mcp::{str_prop, tool};
use crate::spec::{self, Kind, Part};

use super::spec::{slot_of, spec_of, write, Proposal};
use super::spec_copy::spec_dir;
use super::spec_view::{view, TaskSpec};
use super::spec_work::named_task;
use super::task_context::write_task_context;
use super::AppState;

/// Write what was approved back over a copy changed outside the app
/// (SPEC-22), in one repository, keeping the steps it has ticked, and
/// commit it there.
#[tauri::command]
pub async fn put_back_spec(
    app: tauri::AppHandle,
    task_id: String,
    part: Part,
    checkout_id: String,
) -> Result<TaskSpec> {
    super::blocking(app, move |state| {
        let task = state.config.task(&task_id)?;
        let spec = spec_of(state, &task);
        let kind = spec.kind();
        let slot = slot_of(&spec, part, Some(&checkout_id))?;
        let home = spec
            .homes
            .iter()
            .find(|h| h.checkout.id == checkout_id)
            .ok_or_else(|| Error::NotFound(format!("checkout {checkout_id}")))?;
        let record = spec.record(&slot, kind).ok_or_else(|| Error::Other("nothing was approved to put back".into()))?;
        let text = match (part, home.copy(part, kind)) {
            (Part::Tasks, Some(copy)) => spec::keep_ticks(&record, &copy),
            _ => record,
        };
        let path = home.dir.join(part.file(kind));
        write(&path, &format!("{}\n", text.trim()))?;
        if let Some(rel) = &home.rel {
            let message = format!(
                "Put back the approved {} for {}.",
                part.word(kind),
                task.issue_key.as_deref().unwrap_or(&task.branch)
            );
            git::commit_only(
                Path::new(&home.checkout.path),
                &[format!("{rel}/{}", part.file(kind)).as_str()],
                &message,
            )?;
        }
        write_task_context(state, &task)?;
        Ok(view(state, &task))
    })
    .await
}

/// Turn down a change an agent proposed (SPEC-24): its draft goes, and the
/// agent is told.
#[tauri::command]
pub async fn turn_down_spec_proposal(
    app: tauri::AppHandle,
    task_id: String,
    part: Part,
    checkout_id: Option<String>,
) -> Result<TaskSpec> {
    super::blocking(app, move |state| {
        let task = state.config.task(&task_id)?;
        let spec = spec_of(state, &task);
        let slot = slot_of(&spec, part, checkout_id.as_deref())?;
        spec.save_draft(&slot, None)?;
        if let Some(p) = spec.settle(&slot)? {
            tell_proposer(state, &task, &p, "turned down: work to the spec as it was approved");
        }
        Ok(view(state, &task))
    })
    .await
}

/// Tell the agent that proposed a change what became of it. Not being able
/// to (it has stopped) is no reason to fail what the user did.
pub(super) fn tell_proposer(state: &AppState, task: &Task, proposal: &Proposal, outcome: &str) {
    let Some(pane) = &proposal.pane_id else { return };
    if !state.ptys.info(pane).is_ok_and(|p| p.running) {
        return;
    }
    let what = match &proposal.slot.repo {
        Some(folder) => format!("the tasks of {folder}"),
        None => format!("the {}", proposal.slot.part.word(Kind::Feature)),
    };
    let line = format!("Your proposed change to {what} in the spec was {outcome}.");
    let _ = super::hand_over(state, task, pane, "SPEC_ANSWER.md", &line);
}

/// The MCP tools this file answers: the one that was, until Task 9 takes it
/// away with the file. Running agents may still have it in context.
pub fn spec_tools() -> Vec<Value> {
    vec![tool(
        "spec_propose",
        "Gone: write the spec's file with `spec_write`, or edit it in the spec folder of \
         your task. The answer says where that is.",
        json!({ "task_id": str_prop("Your task's id, from the context file or list_tasks") }),
        Vec::new(),
    )]
}

pub fn spec_owns(name: &str) -> bool {
    name == "spec_propose"
}

/// What `spec_propose` answers now: where to write instead.
fn gone(state: &AppState, args: &Value, caller: Option<&str>) -> Error {
    let place = named_task(state, args, caller)
        .map(|task| spec_dir(state, &task).display().to_string())
        .unwrap_or_else(|_| "the spec folder of your task".into());
    Error::Other(format!("spec_propose is gone: write the file with spec_write, or edit it under {place}."))
}

pub async fn spec_call(app: &AppHandle, args: Value, caller: Option<&str>) -> Result<Value> {
    let caller = caller.map(str::to_string);
    super::blocking(app.clone(), move |state| Err(gone(state, &args, caller.as_deref()))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec_world::world;

    #[test]
    fn spec_propose_points_at_spec_write() {
        let (root, state, _) = world(&["api"]);

        let said = gone(&state, &json!({ "task_id": "t1" }), None).to_string();

        assert!(said.starts_with("spec_propose is gone: write the file with spec_write, or edit it under "), "{said}");
        assert!(said.ends_with(&format!("{}.", root.join("tasks/T-1/spec").display())), "{said}");
        let unknown = gone(&state, &json!({}), None).to_string();
        assert!(unknown.contains("spec_write") && unknown.contains("the spec folder of your task"), "{unknown}");
        std::fs::remove_dir_all(&root).ok();
    }
}
