use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::item_details::{self, DetailsPatch, DetailsTarget};
use crate::commands::{self, AppState};
use crate::error::{Error, Result};

pub(super) fn tools() -> Vec<Value> {
    use super::{bool_prop, str_prop, tool};
    let target = str_prop("Existing Work task id. Omit to edit the caller's Work task or standalone Chat.");
    vec![
        tool(
            "task_rename",
            "Set a local display name without changing the source title. Null clears it. Returns canonical details.",
            json!({
                "task_id": target,
                "name": {"type":["string","null"],"description":"Display name, up to 120 Unicode characters. Null resets it."}
            }),
            vec!["name"],
        ),
        tool(
            "task_set_tag",
            "Assign or remove one local text tag. Returns canonical details. Removing an unassigned tag does nothing.",
            json!({"task_id":target,"tag":str_prop("Nonblank tag, up to 40 Unicode characters."),
                   "assigned":bool_prop("True assigns the tag; false removes it.")}),
            vec!["tag", "assigned"],
        ),
        tool(
            "task_set_color",
            "Set or clear a local item color. Returns canonical details without changing other fields.",
            json!({"task_id":target,"color":{"type":["string","null"],
                "enum":["data-1","data-2","data-3","data-4","data-5","data-6",null],
                "description":"Palette color, or null for no color."}}),
            vec!["color"],
        ),
    ]
}

pub(super) fn owns(name: &str) -> bool {
    matches!(name, "task_rename" | "task_set_tag" | "task_set_color")
}

pub(super) async fn call<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    args: Value,
    caller: Option<&str>,
) -> Result<Value> {
    let edit = match name {
        "task_rename" => Edit::Patch(DetailsPatch {
            display_name: Some(serde_json::from_value(supplied(&args, "name")?.clone())?),
            ..Default::default()
        }),
        "task_set_color" => Edit::Patch(DetailsPatch {
            color: Some(serde_json::from_value(supplied(&args, "color")?.clone())?),
            ..Default::default()
        }),
        "task_set_tag" => Edit::Tag {
            tag: super::required(&args, "tag")?.to_owned(),
            assigned: supplied(&args, "assigned")?
                .as_bool()
                .ok_or_else(|| Error::Other("assigned must be a boolean".into()))?,
        },
        other => return Err(Error::NotFound(format!("tool {other}"))),
    };
    let caller = caller.map(str::to_owned);
    let handle = app.clone();
    commands::blocking(app.clone(), move |state| {
        let target = target(state, &args, caller.as_deref())?;
        let change = match edit {
            Edit::Patch(patch) => item_details::edit_details(&state.config, target, patch)?,
            Edit::Tag { tag, assigned } => item_details::set_tag(&state.config, target, &tag, assigned)?,
        };
        commands::desktop_details::notify_details(&handle, &change);
        Ok(serde_json::to_value(change.details)?)
    })
    .await
}

enum Edit {
    Patch(DetailsPatch),
    Tag { tag: String, assigned: bool },
}

fn supplied<'a>(args: &'a Value, key: &str) -> Result<&'a Value> {
    args.get(key).ok_or_else(|| Error::Other(format!("{key} is required")))
}

fn target(state: &AppState, args: &Value, caller: Option<&str>) -> Result<DetailsTarget> {
    // A malformed explicit target must never turn into an edit of the caller.
    if let Some(id) = args.get("task_id") {
        let id = id
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| Error::Other("task_id must be a nonblank Work task id".into()))?;
        return Ok(DetailsTarget::Task(id.to_owned()));
    }
    let pane = caller.ok_or_else(|| Error::Other("Give task_id or call from an eligible Layer pane.".into()))?;
    let info = state.ptys.info(pane)?;
    if info.kind != crate::pty::PaneKind::Agent {
        return Err(Error::Other("Only an agent pane can infer a details target.".into()));
    }
    if info.task_id == commands::CHAT_TASK_ID {
        if info.checkout_id.is_some() {
            return Err(Error::Other("Only a standalone Chat can edit chat details.".into()));
        }
        // The protected core rechecks saved room eligibility and item existence.
        Ok(DetailsTarget::Chat(info.cwd))
    } else {
        Ok(DetailsTarget::Task(info.task_id))
    }
}

#[cfg(test)]
mod tests;
