//! A task's pull request between draft and ready for review, from its card
//! (PR-15). Before, that meant opening GitHub: Open PRs opens drafts by
//! default, and nothing here could take one out of draft.

use std::time::Instant;

use tauri::State;

use crate::error::Result;

use super::pr_resolver::{stale, Connection, Scope};
use super::AppState;

/// Make the checkout's displayed pull request a draft, or ready
/// for review. Returns whether it is a draft now.
#[tauri::command]
pub async fn github_set_pr_draft(
    state: State<'_, AppState>,
    checkout_id: String,
    target: String,
    draft: bool,
) -> Result<bool> {
    let checkout = state.config.checkout(&checkout_id)?;
    let scope = Scope::capture(&state, &checkout.task_id, Instant::now())?;
    let connection = Connection::load(&state).await?;
    set_draft(&state, &scope, &connection, &checkout_id, &target, draft).await
}

pub(super) async fn set_draft(
    state: &AppState,
    scope: &Scope,
    connection: &Connection,
    checkout_id: &str,
    target: &str,
    draft: bool,
) -> Result<bool> {
    let row = scope.checkout(connection, checkout_id, false).await?;
    let pr = row.require_target(target)?;
    let (owner, repo) = row.slug()?;
    if !scope.is_current(state, connection) {
        return Err(stale());
    }
    let client = connection.client()?;
    let node = client.pull_edit_node(owner, repo, pr).await?;
    if !scope.local_is_current(connection, &[row]).await || !scope.is_current(state, connection) {
        return Err(stale());
    }
    client.set_pull_draft(&node, draft).await
}
