//! `set_base` for agents (TASK-6): what the PR panel's base field does, so
//! an agent that finds a repository measured against the wrong branch can
//! put it right. Before, one saw core-service on `main` while its work went
//! to `development`, and could only ask the user to change it by hand.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::git;
use crate::mcp::{required, str_prop, tool};

use super::AppState;

pub fn base_tools() -> Vec<Value> {
    vec![tool(
        "set_base",
        "Change the branch a task's repository is measured against and opens its next \
         pull request into, as the base field in the app's PR panel does. Use it when a \
         repository's base is wrong, for example `main` where its work goes to \
         `development`. The branch must exist on the repository's remote. An open pull \
         request keeps its own base. Returns the base before and after.",
        json!({
            "task_id": str_prop("Task id from list_tasks"),
            "repo": str_prop("Repository name, as list_repos or task_prs gives it"),
            "base": str_prop("The branch, e.g. development"),
        }),
        vec!["task_id", "repo", "base"],
    )]
}

pub async fn base_call(app: &AppHandle, args: Value) -> Result<Value> {
    let state = app.state::<AppState>();
    let task = state.config.task(required(&args, "task_id")?)?;
    let wanted = required(&args, "repo")?.to_string();
    let base = required(&args, "base")?.trim().to_string();
    let (checkout, project) = state
        .config
        .checkouts_of(&task.id)
        .into_iter()
        .find_map(|c| {
            let p = state.config.project(&c.project_id).ok()?;
            p.name.eq_ignore_ascii_case(&wanted).then_some((c, p))
        })
        .ok_or_else(|| Error::NotFound(format!("{wanted} is not part of this task; see task_prs")))?;
    let (was, id, name) = (checkout.base.clone(), checkout.id.clone(), project.name.clone());
    super::blocking(app.clone(), move |state| {
        // Checked against the remote, so a typo is refused here rather than
        // read later as a branch with every commit of the repository in it.
        let repo = super::repo_for(state, &project);
        git::fetch_bases(&[(repo.clone(), base.clone())]);
        if base.starts_with('-') || git::default_tip(&repo, &base).is_none() {
            return Err(Error::Other(format!("{} has no branch {base:?} on its remote", project.name)));
        }
        super::set_base(state, &checkout.id, &base)
    })
    .await?;
    let now = state.config.checkout(&id)?.base;
    Ok(json!({ "repo": name, "was": was, "base": now }))
}
