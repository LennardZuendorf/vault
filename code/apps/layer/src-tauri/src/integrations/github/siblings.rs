//! Each pull request of a multi-repo task names the others (PR-2).
//!
//! Every PR of a task got the same description and nothing else, so whoever
//! reviewed one side could not see from GitHub that there was another: only
//! the Jira comment joined them up. The list lives in a marked section that
//! the app owns and rewrites; the rest of a description is the author's and
//! is left as written.

use serde_json::{json, Value};

use super::GitHub;
use crate::error::{Error, Result};

pub(super) const START: &str = "<!-- villain-layer:siblings -->";
const END: &str = "<!-- /villain-layer:siblings -->";

/// One pull request of the set.
#[derive(Debug, Clone)]
pub struct Sibling {
    /// The repository's name, as the app shows it.
    pub repo: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub url: String,
    pub id: Option<u64>,
    /// Protected siblings can be linked, but never receive a description edit.
    pub editable: bool,
}

impl GitHub {
    /// List every other sibling on writable PRs. Returns the repos whose
    /// PR could not be updated, and why: the PRs are open either way.
    pub async fn link_siblings(
        &self,
        set: &[Sibling],
        ticket: Option<&str>,
        current: impl Fn() -> bool,
    ) -> Vec<(String, String)> {
        let mut failed = Vec::new();
        for pr in set.iter().filter(|pr| pr.editable) {
            let others: Vec<&Sibling> = set.iter().filter(|o| o.url != pr.url).collect();
            if let Err(e) = self.list_others(pr, &others, ticket, &current).await {
                failed.push((pr.repo.clone(), e.to_string()));
            }
        }
        failed
    }

    async fn list_others(
        &self,
        pr: &Sibling,
        others: &[&Sibling],
        ticket: Option<&str>,
        current: &impl Fn() -> bool,
    ) -> Result<()> {
        let stale = || Error::Other("The pull request set changed. Refresh before linking its descriptions.".into());
        if !current() {
            return Err(stale());
        }
        let path = format!("/repos/{}/{}/pulls/{}", pr.owner, pr.name, pr.number);
        let v = self.json(self.req(reqwest::Method::GET, &path)).await?;
        check_edit_target(&v, &pr.owner, &pr.name, pr.number, pr.id)?;
        let body = v.get("body").and_then(Value::as_str).unwrap_or_default();
        let next = with_siblings(body, others, ticket);
        if next != body {
            if !current() {
                return Err(stale());
            }
            self.json(self.req(reqwest::Method::PATCH, &path).json(&json!({ "body": next }))).await?;
        }
        Ok(())
    }
}

pub(crate) fn check_edit_target(v: &Value, owner: &str, repo: &str, number: u64, id: Option<u64>) -> Result<()> {
    let slug = format!("{owner}/{repo}");
    if v.get("number").and_then(Value::as_u64) != Some(number)
        || v.get("state").and_then(Value::as_str) != Some("open")
        || !v.pointer("/base/repo/full_name").and_then(Value::as_str).is_some_and(|r| r.eq_ignore_ascii_case(&slug))
        || id.is_some_and(|id| v.get("id").and_then(Value::as_u64) != Some(id))
    {
        return Err(Error::Other("GitHub did not return the expected open pull request. Nothing was edited.".into()));
    }
    Ok(())
}

/// `body` with its sibling section replaced by one listing `others`, or
/// added at the end when it has none.
pub fn with_siblings(body: &str, others: &[&Sibling], ticket: Option<&str>) -> String {
    let lead = match ticket {
        Some(key) => format!("Part of {key}, together with:"),
        None => "Opened together with:".to_string(),
    };
    let list: Vec<String> = others.iter().map(|o| format!("- {}: {}", o.repo, o.url)).collect();
    let section = format!("{START}\n---\n{lead}\n{}\n{END}", list.join("\n"));

    let own = match body.find(START) {
        Some(at) => {
            let after = body[at..].find(END).map(|e| at + e + END.len()).unwrap_or(body.len());
            let (before, rest) = (body[..at].trim_end(), body[after..].trim_start());
            [before, rest].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("\n\n")
        }
        None => body.to_string(),
    };
    let own = own.trim_end();
    if own.is_empty() {
        section
    } else {
        format!("{own}\n\n{section}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(repo: &str, number: u64) -> Sibling {
        Sibling {
            repo: repo.into(),
            owner: "acme".into(),
            name: repo.into(),
            number,
            id: None,
            editable: true,
            url: format!("https://github.com/acme/{repo}/pull/{number}"),
        }
    }

    #[test]
    fn a_description_gains_the_other_prs_below_what_was_written() {
        let portal = pr("web", 12);
        let body = with_siblings("Serves the spec.\n", &[&portal], Some("ACME-7"));
        assert_eq!(
            body,
            "Serves the spec.\n\n<!-- villain-layer:siblings -->\n---\nPart of ACME-7, together with:\n\
             - web: https://github.com/acme/web/pull/12\n\
             <!-- /villain-layer:siblings -->"
        );
    }

    #[test]
    fn a_later_repo_replaces_the_list_and_keeps_the_authors_edits() {
        let (portal, infra) = (pr("web", 12), pr("infra", 3));
        let first = with_siblings("Serves the spec.", &[&portal], Some("ACME-7"));
        let edited = first.replace("Serves the spec.", "Serves the spec, and says so.");

        let second = with_siblings(&edited, &[&portal, &infra], Some("ACME-7"));

        assert!(second.starts_with("Serves the spec, and says so.\n\n"));
        assert_eq!(second.matches("villain-layer:siblings -->").count(), 2);
        assert!(second.contains("infra: https://github.com/acme/infra/pull/3"));
        assert_eq!(with_siblings(&second, &[&portal, &infra], Some("ACME-7")), second);
    }

    #[test]
    fn text_written_after_the_list_stays() {
        let portal = pr("web", 12);
        let body = format!("{}\n\nThanks!", with_siblings("Spec.", &[&portal], None));

        let again = with_siblings(&body, &[&portal], None);

        assert!(again.starts_with("Spec.\n\nThanks!\n\n<!-- villain-layer:siblings -->"));
        assert!(again.contains("Opened together with:"));
    }

    #[tokio::test]
    async fn a_sibling_closed_during_publication_does_not_get_its_description_patched() {
        use axum::{http::Method, response::IntoResponse, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let patches = Arc::new(AtomicUsize::new(0));
        let seen = patches.clone();
        let app = Router::new().fallback(move |method: Method, uri: axum::http::Uri| {
            let seen = seen.clone();
            async move {
                if method == Method::PATCH {
                    seen.fetch_add(1, Ordering::Relaxed);
                }
                let (number, name) = if uri.path().ends_with("/12") { (12, "web") } else { (15, "api") };
                Json(json!({"number": number, "state": if number == 12 {"closed"} else {"open"},
                    "base":{"repo":{"full_name":format!("acme/{name}")}}, "body":"Original"}))
                .into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = GitHub::new(&crate::config::GithubConfig { api_url, ..Default::default() }, "fixture").unwrap();
        let failures = client.link_siblings(&[pr("web", 12), pr("api", 15)], None, || true).await;
        assert_eq!(patches.load(Ordering::Relaxed), 1, "only the still-open PR can be edited");
        assert_eq!(failures.first().map(|(repo, _)| repo.as_str()), Some("web"));
    }

    #[tokio::test]
    async fn round_one_a_scope_change_inside_the_preliminary_get_prevents_every_patch() {
        use axum::{
            http::{Method, StatusCode},
            response::IntoResponse,
            Json, Router,
        };
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };
        let current = Arc::new(AtomicBool::new(true));
        let changes = current.clone();
        let patches = Arc::new(AtomicUsize::new(0));
        let log = patches.clone();
        let app = Router::new().fallback(move |method: Method| {
            let (changes, log) = (changes.clone(), log.clone());
            async move {
                if method == Method::PATCH {
                    log.fetch_add(1, Ordering::SeqCst);
                    return StatusCode::OK.into_response();
                }
                changes.store(false, Ordering::SeqCst);
                Json(json!({"number":12,"id":99,"state":"open","base":{"repo":{"full_name":"acme/web"}},"body":"Original"})).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = GitHub::new(&crate::config::GithubConfig { api_url, ..Default::default() }, "fixture").unwrap();
        let failures = client
            .link_siblings(&[Sibling { id: Some(99), ..pr("web", 12) }, pr("api", 15)], None, || {
                current.load(Ordering::SeqCst)
            })
            .await;
        assert_eq!(patches.load(Ordering::SeqCst), 0);
        assert_eq!(failures.len(), 2);
    }

    #[tokio::test]
    async fn round_two_a_new_pr_links_its_pinned_sibling_without_editing_the_pin() {
        use axum::{
            body::Bytes,
            http::{Method, Uri},
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        let patches = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let log = patches.clone();
        let app = Router::new().fallback(move |method: Method, uri: Uri, body: Bytes| {
            let log = log.clone();
            async move {
                if method == Method::PATCH {
                    log.lock().unwrap().push((uri.path().into(), serde_json::from_slice(&body).unwrap()));
                }
                if method == Method::POST {
                    assert_eq!(uri.path(), "/repos/acme/api/pulls");
                    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["head"], "task");
                }
                let (name, number) = if uri.path().ends_with("/12") { ("web", 12) } else { ("api", 15) };
                Json(json!({"number":number,"state":"open","base":{"repo":{"full_name":format!("acme/{name}")}},
                    "html_url":format!("https://github.com/acme/{name}/pull/{number}"),"body":"Original"}))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = GitHub::new(&crate::config::GithubConfig { api_url, ..Default::default() }, "fixture").unwrap();
        let a = Sibling { editable: false, ..pr("web", 12) };
        let newly_open = client.create_pull("acme", "api", "Change", "Original", "task", "main", false).await.unwrap();
        let b = Sibling { url: newly_open.url, ..pr("api", newly_open.number) };
        let failures = client.link_siblings(&[a, b], None, || true).await;
        assert!(failures.is_empty(), "{failures:?}");
        let patches = patches.lock().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, "/repos/acme/api/pulls/15");
        let body = patches[0].1["body"].as_str().unwrap();
        assert!(body.starts_with("Original"));
        assert!(body.contains("- web: https://github.com/acme/web/pull/12"));
    }
}
