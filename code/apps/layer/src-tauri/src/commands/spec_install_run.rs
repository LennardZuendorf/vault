//! Installing a task's spec toolset in the background (§19) and saying how it
//! went: where a task is made there is no app to announce with, so the app's
//! handle is kept from launch, and each install is announced as `spec:install`.

use std::sync::OnceLock;

use tauri::{AppHandle, Emitter, Manager};

use crate::config::Task;
use crate::error::Result;

use super::panes::agent_file_dir;
use super::spec_copy::toolset_of;
use super::spec_install::{install, Announce, InstallState, INSTALL_RECORD};
use super::spec_runner::Real;
use super::AppState;

static APP: OnceLock<AppHandle> = OnceLock::new();

/// From launch on, installs can be started where a task is made, which has
/// no app of its own to announce with. Nothing starts in tests.
pub fn attach_spec_install(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Install `task_id`'s toolset in the background and announce how it goes.
pub(crate) fn start(task_id: &str) {
    let Some(app) = APP.get().cloned() else { return };
    let id = task_id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = announce(app, id.clone()).await {
            eprintln!("the spec toolset of {id} was not installed: {e}");
        }
    });
}

/// `start`, for a task that gained a repository and has no install on
/// record: the first repository of a task that had none.
pub(crate) fn start_unless_installed(state: &AppState, task: &Task) {
    if agent_file_dir(state, task).is_some_and(|dir| std::fs::symlink_metadata(dir.join(INSTALL_RECORD)).is_err()) {
        start(&task.id);
    }
}

fn emit(app: &AppHandle, task_id: &str, state: &InstallState) {
    let _ = app.emit("spec:install", Announce { task_id, state });
}

/// Install, saying so when it starts and when it is done.
async fn announce(app: AppHandle, task_id: String) -> Result<InstallState> {
    let starting = {
        let state = app.state::<AppState>();
        let task = state.config.task(&task_id)?;
        let toolset = toolset_of(&state, &task);
        InstallState { toolset: toolset.id, version: toolset.version, ok: false, error: None, running: true }
    };
    emit(&app, &task_id, &starting);
    let id = task_id.clone();
    // The steps wait on npm and on git: off the runtime's threads.
    let done = super::blocking(app.clone(), move |state| Ok(install(state, &state.config.task(&id)?, &Real))).await?;
    // Running already: that install announces its own end.
    if !done.running {
        emit(&app, &task_id, &done);
    }
    Ok(done)
}

/// Install the task's toolset again: Retry on the Spec tab. The state it
/// reaches is also announced, as `spec:install`.
#[tauri::command]
pub async fn install_spec_toolset(app: AppHandle, task_id: String) -> Result<InstallState> {
    announce(app, task_id).await
}
