//! Apply checkout evidence only while the backing worktree and association
//! still match. Saving UPD-6 does not require notifying anyone of a sweep.

use super::*;
use crate::commands::{github, henchman_watch};

impl Scope {
    pub(in crate::commands) fn accept_unread(
        &self,
        state: &AppState,
        connection: Option<&Connection>,
        reason: String,
    ) -> Result<Vec<CheckoutPr>> {
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        state.config.with(|config| {
            let current = connection.map_or_else(|| config.github.is_none(), |c| c.is_current(config, *generation));
            let mut read_at = state.prs_read_at.lock();
            if !current || !self.matches(config) || read_at.get(&self.task.id).is_some_and(|at| *at > self.began) {
                return Err(stale());
            }
            read_at.insert(self.task.id.clone(), self.began);
            state.prs_settled.lock().remove(&self.task.id);
            if self.checkouts.is_empty() {
                return Err(Error::Other(reason));
            }
            Ok(self
                .checkouts
                .iter()
                .map(|input| {
                    let mut row = CheckoutPr::unread(&input.checkout, &input.name);
                    row.error = Some(reason.clone());
                    row
                })
                .collect())
        })
    }

    /// Runs on the blocking pool. Finish passes only the checkout being
    /// removed once earlier worktrees may no longer exist.
    pub(in crate::commands) fn require_branch_evidence(&self, rows: &[Resolved]) -> Result<()> {
        for row in rows {
            check_branches(
                &row.checkout,
                &self.task.branch,
                row.branches.as_ref(),
                self.history_for(&row.checkout.id),
            )?;
        }
        Ok(())
    }

    fn history_for(&self, checkout_id: &str) -> Option<&HistoryScope> {
        self.checkouts.iter().find(|i| i.checkout.id == checkout_id).map(|i| &i.history)
    }

    pub(in crate::commands) async fn local_is_current(&self, connection: &Connection, rows: &[Resolved]) -> bool {
        recheck(self, connection, rows).await.is_ok_and(|checks| checks.iter().all(Result::is_ok))
    }

    pub(in crate::commands) async fn advance_rows(
        &mut self,
        state: &AppState,
        connection: &Connection,
        rows: &mut [Resolved],
    ) -> Result<()> {
        let local = recheck(self, connection, rows).await?;
        for (row, check) in rows.iter_mut().zip(local) {
            if let Err(e) = check {
                row.row.error.get_or_insert_with(|| e.to_string());
                row.row.action_target = None;
                row.advance = None;
            }
        }
        let advanced: Vec<_> = rows
            .iter()
            .filter(|r| r.row.error.is_none())
            .filter_map(|r| r.advance.as_ref().map(|point| (r.checkout.id.clone(), point.clone())))
            .collect();
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        if advanced.is_empty() {
            return state
                .config
                .with(|c| self.accepts(state, c, connection, *generation))
                .then_some(())
                .ok_or_else(stale);
        }
        state.config.update_local_tasks_if(|config| {
            if !self.accepts(state, config, connection, *generation) {
                return Err(stale());
            }
            for (id, point) in &advanced {
                if let Some(checkout) = config.checkouts.iter_mut().find(|c| &c.id == id) {
                    checkout.base_commit = Some(point.clone());
                }
            }
            Ok(((), true))
        })?;
        for (id, point) in advanced {
            if let Some(input) = self.checkouts.iter_mut().find(|c| c.checkout.id == id) {
                input.checkout.base_commit = Some(point.clone());
            }
            if let Some(row) = rows.iter_mut().find(|r| r.checkout.id == id) {
                row.checkout.base_commit = Some(point);
            }
            state.status_cache.lock().remove(&id);
        }
        Ok(())
    }

    pub(in crate::commands) async fn accept_rows(
        &mut self,
        state: &AppState,
        connection: &Connection,
        rows: &mut [Resolved],
        sweep: bool,
    ) -> Result<Vec<CheckoutPr>> {
        self.advance_rows(state, connection, rows).await?;
        let rows: Vec<_> = rows.iter().map(|r| r.row.clone()).collect();
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        let before = state.config.with(|config| {
            if !self.accepts(state, config, connection, *generation) {
                return Err(stale());
            }
            state.prs_read_at.lock().insert(self.task.id.clone(), self.began);
            let mut settled = state.prs_settled.lock();
            if sweep && github::settled(&rows) {
                settled.insert(self.task.id.clone(), Instant::now());
            } else if !github::settled(&rows) {
                settled.remove(&self.task.id);
            }
            Ok(if sweep { state.prs_seen.lock().insert(self.task.id.clone(), rows.clone()) } else { None })
        })?;
        drop(generation);
        if let Some(before) = before.filter(|_| self.is_current(state, connection)) {
            henchman_watch::seen(state, &self.task.id, &before, &rows);
        }
        Ok(rows)
    }
}

async fn recheck(scope: &Scope, connection: &Connection, rows: &[Resolved]) -> Result<Vec<Result<()>>> {
    let branch = scope.task.branch.clone();
    let config = connection.config.clone();
    let evidence: Vec<_> = rows
        .iter()
        .map(|r| {
            let history = scope.history_for(&r.checkout.id).cloned();
            (r.checkout.clone(), r.local.clone(), r.branches.clone(), history, r.row.error.is_none())
        })
        .collect();
    off_runtime(move || {
        evidence
            .into_iter()
            .map(|(checkout, before, queried, history, complete)| {
                if !complete {
                    return Ok(());
                }
                if Some(Local::read(&checkout, &branch, &config)?) != before {
                    return Err(stale());
                }
                check_branches(&checkout, &branch, queried.as_ref(), history.as_ref())
            })
            .collect()
    })
    .await
}

fn check_branches(
    checkout: &Checkout,
    branch: &str,
    queried: Option<&BranchStamp>,
    history: Option<&HistoryScope>,
) -> Result<()> {
    // A topic opened and left during the read need not change HEAD, the task
    // ref, or status, but it changes what we queried.
    let actual = pr_branches::branch_evidence(checkout, branch, history.ok_or_else(stale)?);
    if !actual.errors.is_empty() || Some(&actual.stamp()) != queried {
        return Err(stale());
    }
    Ok(())
}
