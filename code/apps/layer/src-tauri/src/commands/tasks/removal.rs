//! Remove only the authorized worktrees, then reconcile what Git removed.

use super::*;
use crate::commands::pr_resolver::{stale, Connection, Resolved, Scope};
use crate::config::FinishReceipt;

pub(in crate::commands) fn finish_worktrees(
    state: &AppState,
    scope: &Scope,
    connection: &Connection,
    evidence: &[Resolved],
    before_remove: impl Fn(Option<&Checkout>) -> Result<()>,
) -> Result<Vec<RepoResult>> {
    let receipt = state
        .config
        .with(|config| config.finish_receipts.get(&scope.task.id).filter(|r| r.matches(config, &scope.task)).cloned())
        .filter(FinishReceipt::worktree_still_absent);
    let recovering = !evidence.iter().any(|r| r.row.covered_merge) && receipt.is_some();
    crate::commands::finish_tool::validate_finish(evidence, recovering)?;
    let config_current = |config: &AppConfig| {
        scope.matches(config) && (!recovering || config.finish_receipts.get(&scope.task.id) == receipt.as_ref())
    };
    let receipt_current = |config: &AppConfig| {
        !recovering || receipt.as_ref().is_some_and(|r| r.matches(config, &scope.task) && r.worktree_still_absent())
    };
    delete_task_inner(
        state,
        scope.task.id.clone(),
        false,
        true,
        |checkout| {
            before_remove(checkout)?;
            if recovering && !state.config.with(|c| config_current(c) && receipt_current(c)) {
                return Err(stale());
            }
            let evidence = match checkout {
                None => evidence,
                Some(checkout) => {
                    std::slice::from_ref(evidence.iter().find(|r| r.checkout.id == checkout.id).ok_or_else(stale)?)
                }
            };
            scope.require_branch_evidence(evidence)?;
            scope.require_current(state, connection, evidence)
        },
        |config| (config_current(config) && receipt_current(config)).then_some(()).ok_or_else(stale),
        |checkout| {
            let row = evidence.iter().find(|r| r.checkout.id == checkout.id)?;
            let number = row.covered_by?;
            let pr = row.row.pulls().find(|pr| pr.number == number && pr.merged)?;
            FinishReceipt::new(&scope.task, &connection.config, checkout, row.slug().ok()?, pr)
        },
    )
}

pub(super) fn remove_worktrees(
    state: &AppState,
    checkouts: Vec<Checkout>,
    force: bool,
    finished: bool,
    before_remove: impl Fn(Option<&Checkout>) -> Result<()>,
) -> (Vec<RepoResult>, Vec<Checkout>) {
    let mut results = Vec::new();
    let mut removed = Vec::new();
    for checkout in checkouts {
        let project = match state.config.project(&checkout.project_id) {
            Ok(p) => p,
            Err(e) if finished => {
                results.push(kept(&checkout, "(unknown)".into(), e));
                break;
            }
            // Ordinary Delete drops the record when the project is gone.
            Err(_) => continue,
        };
        let path = Path::new(&checkout.path);
        let home = super::super::owner_of(&project, &checkout.path);
        if let Err(e) = before_remove(Some(&checkout)) {
            results.push(kept(&checkout, project.name, e));
            break;
        }
        let (ok, detail) = if !path.exists() {
            // Already removed by hand; just tidy the admin files, wherever
            // it was registered. This isn't a removal we can reconcile.
            let _ = git::prune_worktrees(&project.repo());
            let _ = git::prune_worktrees(Path::new(&project.path));
            (true, "already gone".to_string())
        } else {
            match git::remove_worktree(&home, &checkout.path, force) {
                Ok(()) => {
                    if finished {
                        removed.push(checkout.clone());
                    }
                    (true, "removed".to_string())
                }
                Err(e) => (false, e.to_string()),
            }
        };
        results.push(RepoResult { checkout_id: checkout.id, repo: project.name, ok, detail });
    }
    (results, removed)
}

fn kept(checkout: &Checkout, repo: String, error: Error) -> RepoResult {
    RepoResult { checkout_id: checkout.id.clone(), repo, ok: false, detail: format!("Worktree kept: {error}") }
}

/// A later refusal cannot bring the first worktree back. Remove only its
/// exact record, and leave any replacement or newly added checkout alone.
pub(super) fn reconcile_removed(
    state: &AppState,
    removed: &[Checkout],
    receipt: impl Fn(&Checkout) -> Option<FinishReceipt>,
) -> Result<()> {
    if removed.is_empty() {
        return Ok(());
    }
    let cleared = state.config.update_local_tasks_if(|config| {
        let mut ids = Vec::new();
        let mut receipts = Vec::new();
        config.checkouts.retain(|current| {
            let gone = removed.iter().any(|was| {
                was.id == current.id
                    && was.task_id == current.task_id
                    && was.project_id == current.project_id
                    && was.path == current.path
                    && std::fs::symlink_metadata(&was.path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            });
            if gone {
                ids.push(current.id.clone());
                if let Some(receipt) = receipt(current) {
                    receipts.push((current.task_id.clone(), receipt));
                }
            }
            !gone
        });
        for (id, receipt) in receipts {
            if config.tasks.iter().any(|task| task.id == id && receipt.matches(config, task)) {
                config.finish_receipts.insert(id, receipt);
            }
        }
        let changed = !ids.is_empty();
        Ok((ids, changed))
    })?;
    state.status_cache.lock().retain(|id, _| !cleared.contains(id));
    Ok(())
}
