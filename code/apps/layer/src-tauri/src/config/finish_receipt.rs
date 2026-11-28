//! A local record of the verified merge behind a partial Finish. This is
//! not a PR read for any checkout that still exists.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{AppConfig, Checkout, GithubConfig, Task};
use crate::integrations::github::PullRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FinishReceipt {
    task_created_at: chrono::DateTime<chrono::Utc>,
    task_root: String,
    task_branch: String,
    checkout_id: String,
    project_id: String,
    path: String,
    api_url: String,
    web_url: String,
    repo: String,
    pr_id: u64,
    pr_number: u64,
    head_sha: String,
}

impl FinishReceipt {
    pub(crate) fn new(
        task: &Task,
        github: &GithubConfig,
        checkout: &Checkout,
        repo: (&str, &str),
        pr: &PullRequest,
    ) -> Option<Self> {
        Some(Self {
            task_created_at: task.created_at,
            task_root: task.root.clone(),
            task_branch: task.branch.clone(),
            checkout_id: checkout.id.clone(),
            project_id: checkout.project_id.clone(),
            path: checkout.path.clone(),
            api_url: github.api_url.clone(),
            web_url: github.web_url.clone(),
            repo: format!("{}/{}", repo.0, repo.1),
            pr_id: pr.id?,
            pr_number: pr.number,
            head_sha: pr.head_sha.clone(),
        })
    }

    pub(crate) fn matches(&self, config: &AppConfig, task: &Task) -> bool {
        self.task_created_at == task.created_at
            && self.task_root == task.root
            && self.task_branch == task.branch
            && config.github.as_ref().is_some_and(|g| g.api_url == self.api_url && g.web_url == self.web_url)
            && !config.checkouts.iter().any(|c| c.id == self.checkout_id || c.path == self.path)
    }

    pub(crate) fn worktree_still_absent(&self) -> bool {
        std::fs::symlink_metadata(Path::new(&self.path)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    }
}
