//! Read GitHub details that establish a local association, without redirects.

use std::num::NonZeroU64;
use std::sync::LazyLock;

use reqwest::{redirect::Policy, Client, Method, Response, StatusCode};
use serde_json::Value;

use super::{to_pr, valid_repo_segment, GitHub, PullRequest};
use crate::config::GithubConfig;
use crate::error::{Error, Result};
use crate::integrations::http_client_builder;

impl GitHub {
    pub(crate) async fn pin_pull(
        &self,
        cfg: &GithubConfig,
        owner: &str,
        repo: &str,
        number: u64,
        expected_id: Option<NonZeroU64>,
    ) -> Result<(PullRequest, String, NonZeroU64)> {
        if self.api_url != cfg.api_url.trim_end_matches('/') {
            return Err(Error::Other("The GitHub connection changed. Try again.".into()));
        }
        if number == 0 || ![owner, repo].into_iter().all(valid_repo_segment) {
            return Err(Error::Other("The GitHub repository or pull request number is not valid.".into()));
        }
        let v = self.verified_get(&format!("/repos/{owner}/{repo}/pulls/{number}")).await?;
        let id = v
            .get("id")
            .and_then(Value::as_u64)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| Error::Other("GitHub did not return a stable pull request ID. Try again.".into()))?;
        if expected_id.is_some_and(|expected| expected != id) {
            return Err(Error::PrPinIdentityChanged);
        }
        let pr = to_pr(&v);
        let slug = v.pointer("/base/repo/full_name").and_then(Value::as_str).unwrap_or_default();
        if pr.number != number || !slug.eq_ignore_ascii_case(&format!("{owner}/{repo}")) {
            return Err(Error::Other("GitHub did not return the requested pull request for this repository.".into()));
        }
        if !matches!(pr.state.as_str(), "open" | "closed")
            || v.get("merged").and_then(Value::as_bool).is_none()
            || pr.merged && pr.state != "closed"
            || pr.head.is_empty()
            || pr.head_sha.is_empty()
            || pr.base.is_empty()
        {
            return Err(Error::Other("GitHub returned incomplete pull request details. Try again.".into()));
        }
        Ok((pr, slug.to_owned(), id))
    }

    pub(crate) async fn connection_login(&self) -> Result<String> {
        self.verified_get("/user")
            .await?
            .get("login")
            .and_then(Value::as_str)
            .filter(|login| !login.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Other("GitHub did not return a login. Check the connection.".into()))
    }

    async fn verified_get(&self, path: &str) -> Result<Value> {
        // A redirect cannot prove which API endpoint accepted the token.
        static CLIENT: LazyLock<reqwest::Result<Client>> =
            LazyLock::new(|| http_client_builder().redirect(Policy::none()).build());
        let unreadable = || Error::Other("Could not read GitHub's response. Try again.".into());
        let client = CLIENT.as_ref().map_err(|_| unreadable())?;
        let request = self.req(Method::GET, path).build().map_err(|_| unreadable())?;
        let response = client.execute(request).await.map_err(|_| unreadable())?;
        let status = response.status();
        match status {
            StatusCode::OK => response.json().await.map_err(|_| unreadable()),
            StatusCode::UNAUTHORIZED => Err(Error::TokenRejected("GitHub")),
            StatusCode::FORBIDDEN if needs_sso(response).await => Err(Error::TokenNeedsSso("GitHub")),
            // A proxy's diagnostic can echo credentials. Do not relay the
            // body, headers, or redirect location to agents.
            _ => Err(Error::Other(format!("GitHub could not verify the request ({status}). Check the connection."))),
        }
    }
}

async fn needs_sso(mut response: Response) -> bool {
    if response.headers().get("x-github-sso").and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("required")) {
        return true;
    }
    const BODY_LIMIT: usize = 4096;
    const MARKER: &[u8] = b"SAML enforcement";
    let mut prefix = Vec::new();
    while prefix.len() < BODY_LIMIT {
        let Ok(Some(chunk)) = response.chunk().await else { break };
        prefix.extend_from_slice(&chunk[..chunk.len().min(BODY_LIMIT - prefix.len())]);
        if prefix.windows(MARKER.len()).any(|bytes| bytes == MARKER) {
            return true;
        }
    }
    false
}
