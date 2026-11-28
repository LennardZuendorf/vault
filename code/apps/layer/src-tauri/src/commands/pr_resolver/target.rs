//! A displayed primary PR is an authorization boundary for actions on its
//! card. A different primary or pin revision needs a fresh user choice.

use sha2::{Digest, Sha256};

use super::*;

pub(in crate::commands) fn action_target(
    connection: &Connection,
    checkout: &Checkout,
    slug: (&str, &str),
    pr: &PullRequest,
) -> Option<String> {
    let id = pr.id?;
    let payload = serde_json::to_vec(&(
        &connection.config.api_url,
        &connection.config.web_url,
        connection.generation,
        &checkout.id,
        &checkout.task_id,
        &checkout.project_id,
        &checkout.path,
        &checkout.base,
        checkout.pr_pin_revision,
        slug.0.to_ascii_lowercase(),
        slug.1.to_ascii_lowercase(),
        pr.number,
        id,
    ))
    .ok()?;
    Some(hex::encode(Sha256::digest(payload)))
}

impl Resolved {
    pub(in crate::commands) async fn require_repo_current(&self, connection: &Connection) -> Result<()> {
        let path = self.checkout.path.clone();
        let config = connection.config.clone();
        let current = off_runtime(move || pr_branches::configured_checkout_repo(Path::new(&path), &config)).await??;
        (self.slug.as_ref() == Some(&current)).then_some(()).ok_or_else(stale)
    }

    pub(in crate::commands) fn require_target(&self, expected: &str) -> Result<&PullRequest> {
        self.require_complete()?;
        self.row.open().filter(|_| self.row.action_target.as_deref() == Some(expected)).ok_or_else(|| {
            Error::Other("The selected pull request or its pin changed. Refresh and review the action again.".into())
        })
    }
}
