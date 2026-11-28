//! Bounded branch reads for the checkout PR resolver.

use reqwest::header::{HeaderMap, LINK};

use super::{to_pr, GitHub, PullRequest};
use crate::error::{Error, Result};

const MAX_PAGES: u32 = 5;
const PAGE_SIZE: u32 = 100;

#[derive(Default)]
pub(crate) struct BranchPulls {
    pub prs: Vec<PullRequest>,
    pub error: Option<Error>,
}

impl GitHub {
    /// Not a `Result`: later failures must not throw out completed pages.
    pub(crate) async fn paged_pulls_for_branch(&self, owner: &str, repo: &str, branch: &str) -> BranchPulls {
        let mut found = BranchPulls::default();
        if ![owner, repo].into_iter().all(valid_repo_segment) {
            found.error = Some(Error::Other("The GitHub repository name is not valid.".into()));
            return found;
        }
        for page in 1..=MAX_PAGES {
            match self.pull_page(owner, repo, branch, page).await {
                Ok((items, next)) => {
                    found.prs.extend(items.prs);
                    found.error = found.error.or(items.error);
                    match next {
                        Ok(false) => break,
                        Ok(true) if page < MAX_PAGES => {}
                        Ok(true) => {
                            found.error = Some(Error::Other(format!(
                                "GitHub pull requests for {branch} exceed the read limit of {MAX_PAGES} pages."
                            )));
                        }
                        Err(e) => {
                            found.error = Some(e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    found.error = Some(e);
                    break;
                }
            }
        }
        found
    }

    async fn pull_page(&self, owner: &str, repo: &str, branch: &str, page: u32) -> Result<(BranchPulls, Result<bool>)> {
        // Use Link only as a signal. GitHub can name a canonical repository
        // URL in Link; neither its host nor its path gets our Authorization.
        let res = self
            .req(reqwest::Method::GET, &format!("/repos/{owner}/{repo}/pulls"))
            .query(&[("state", "all"), ("head", &format!("{owner}:{branch}"))])
            .query(&[("per_page", PAGE_SIZE), ("page", page)])
            .send()
            .await?;
        let next = has_next(res.headers());
        let value = Self::read_json(res).await?;
        let records = value.as_array().ok_or_else(|| malformed(branch))?;
        let mut items = BranchPulls::default();
        for pr in records.iter().map(to_pr) {
            if pr.number > 0
                && matches!(pr.state.as_str(), "open" | "closed")
                && !pr.head.is_empty()
                && !pr.head_sha.is_empty()
                && !pr.base.is_empty()
                && !pr.url.is_empty()
            {
                items.prs.push(pr);
            } else {
                items.error.get_or_insert_with(|| malformed(branch));
            }
        }
        Ok((items, next))
    }
}

pub(crate) fn valid_repo_segment(s: &str) -> bool {
    !matches!(s, "" | "." | "..") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn malformed(branch: &str) -> Error {
    Error::Other(format!("GitHub returned malformed pull request data for {branch}."))
}

fn has_next(headers: &HeaderMap) -> Result<bool> {
    let malformed = || Error::Other("GitHub returned malformed pull request pagination.".into());
    let mut next = false;
    for value in headers.get_all(LINK) {
        let mut links = value.to_str().map_err(|_| malformed())?.trim();
        while !links.is_empty() {
            let (_, params) = links.strip_prefix('<').and_then(|s| s.split_once('>')).ok_or_else(malformed)?;
            let (params, rest) = params.split_once(',').unwrap_or((params, ""));
            let rel = params.split(';').find_map(|p| p.trim().strip_prefix("rel=")).ok_or_else(malformed)?;
            next |= rel.trim_matches('"').split_ascii_whitespace().any(|rel| rel == "next");
            links = rest.trim_start();
        }
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use axum::{
        extract::Query,
        response::{IntoResponse, Response},
        routing::get,
        Json, Router,
    };
    use reqwest::{header::HeaderMap, StatusCode};
    use std::collections::HashMap;

    const HEAD: &str = "topic/#one+two";
    const NEXT: &str = "<http://127.0.0.1:1/not-the-configured-api>; rel=\"next\"";

    async fn fixture(page: impl Fn(u32) -> Response + Send + Sync + 'static) -> GitHub {
        let page = std::sync::Arc::new(page);
        let app = Router::new().route(
            "/api/v3/repos/acme/tool/pulls",
            get(move |Query(query): Query<HashMap<String, String>>, headers: HeaderMap| {
                let page = page.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer local-fixture-token");
                    assert_eq!(query["state"], "all");
                    assert_eq!(query["head"], format!("acme:{HEAD}"));
                    page(query.get("page").map(|n| n.parse().unwrap()).unwrap_or(1))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}/api/v3", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        GitHub { api_url, token: "local-fixture-token".into(), client: http_client() }
    }

    fn item(n: u32, merged: bool) -> Value {
        json!({
            "number": n, "title": "A change", "state": if merged { "closed" } else { "open" },
            "draft": false, "user": {"login": "dev"},
            "head": {"ref": HEAD, "sha": "1111111111111111111111111111111111111111"},
            "base": {"ref": "main"}, "html_url": format!("https://git.example/acme/tool/pull/{n}"),
            "merged_at": if merged { Some("2026-01-01T00:00:00Z") } else { None },
        })
    }

    async fn read(client: &GitHub) -> (Vec<PullRequest>, Option<Error>) {
        let BranchPulls { prs, error } = client.paged_pulls_for_branch("acme", "tool", HEAD).await;
        (prs, error)
    }

    #[tokio::test]
    async fn a_next_link_pages_on_the_configured_api_even_after_a_short_page() {
        let client = fixture(|page| match page {
            1 => ([("link", NEXT)], Json(json!([item(9, false)]))).into_response(),
            2 => Json(json!([item(5, true)])).into_response(),
            _ => panic!("unexpected page {page}"),
        })
        .await;
        let (prs, error) = read(&client).await;
        assert!(error.is_none(), "{error:?}");
        assert_eq!(prs.iter().map(|pr| (pr.number, pr.merged)).collect::<Vec<_>>(), [(9, false), (5, true)]);
    }

    #[tokio::test]
    async fn a_later_auth_failure_reports_its_type_without_losing_earlier_pages() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            let client = fixture(move |page| match page {
                1 => ([("link", NEXT)], Json(json!([item(9, false)]))).into_response(),
                2 => (status, [("x-github-sso", "required")], "access denied").into_response(),
                _ => panic!("unexpected page {page}"),
            })
            .await;
            let (prs, error) = read(&client).await;
            assert_eq!(prs.iter().map(|pr| pr.number).collect::<Vec<_>>(), [9]);
            match status {
                StatusCode::UNAUTHORIZED => assert!(matches!(error, Some(Error::TokenRejected("GitHub")))),
                _ => assert!(matches!(error, Some(Error::TokenNeedsSso("GitHub")))),
            }
        }
    }

    #[tokio::test]
    async fn a_branch_page_cap_keeps_read_prs_and_marks_the_read_incomplete() {
        let client = fixture(|page| ([("link", NEXT)], Json(json!([item(page, false)]))).into_response()).await;
        let (prs, error) = read(&client).await;
        assert_eq!(prs.iter().map(|pr| pr.number).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        assert!(error.unwrap().to_string().contains("5 pages"));
    }

    #[tokio::test]
    async fn an_invalid_record_does_not_discard_good_records_or_look_complete() {
        let client = fixture(|_| Json(json!([item(9, false), null])).into_response()).await;
        let (prs, error) = read(&client).await;
        assert_eq!(prs.iter().map(|pr| pr.number).collect::<Vec<_>>(), [9]);
        assert!(error.unwrap().to_string().contains("malformed"));
    }
}
