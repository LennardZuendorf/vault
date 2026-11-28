//! Verify a checkout PR against its configured GitHub connection before
//! associating it with the task (PR-17).

use std::path::Path;

use reqwest::Url;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::config::{AppConfig, GithubConfig, PrPin, PrPinIdentity, StoredPrPin, TrackerKind};
use crate::error::{Error, Result};
use crate::integrations::github::{valid_repo_segment, GitHub, PullRequest};
use crate::mcp::{confirm_prop, required, str_prop, tool};
use crate::secrets;

use super::{pr_branches::configured_checkout_repo, AppState};

/// A GitHub client may read a pin only after proving the local credential
/// belongs to the configured API origin. Workspace settings are not proof.
pub(crate) struct BoundGithub {
    client: GitHub,
    config: GithubConfig,
    api_origin: String,
    generation: u64,
}

impl BoundGithub {
    /// Keychain access can block. Call on the blocking pool.
    pub(crate) fn load(state: &AppState) -> Result<Self> {
        Self::load_with(state, secrets::github_credential)
    }

    fn load_with(state: &AppState, stored: impl FnOnce() -> Result<secrets::GithubCredential>) -> Result<Self> {
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        let config = state.config.with(|c| c.github.clone()).ok_or(Error::NotConfigured("GitHub"))?;
        Self::with_credential(config, *generation, stored()?)
    }

    pub(super) fn with_credential(
        config: GithubConfig,
        generation: u64,
        stored: secrets::GithubCredential,
    ) -> Result<Self> {
        let api_origin = api_origin(&config)?;
        let token = stored.for_origin(&api_origin)?;
        let client = GitHub::new(&config, &token)?;
        Ok(Self { client, config, api_origin, generation })
    }

    pub(crate) fn config(&self) -> &GithubConfig {
        &self.config
    }

    pub(crate) fn client(&self) -> &GitHub {
        &self.client
    }

    /// Return only a locally valid link, even if the server cannot read it.
    /// `repo` must come from `configured_checkout_repo` using this snapshot.
    pub(crate) fn checked_pin_url(&self, repo: &str, saved: &StoredPrPin) -> Result<String> {
        display_pin(self.config(), repo, saved)
    }

    /// Compare under the credential lock and against the config being used
    /// for the local update, not a separate earlier read.
    pub(crate) fn is_current(&self, config: &AppConfig, generation: u64) -> bool {
        generation == self.generation
            && config
                .github
                .as_ref()
                .is_some_and(|g| g.api_url == self.config.api_url && g.web_url == self.config.web_url)
    }
}

#[derive(Serialize)]
struct LinkedPr {
    checkout_id: String,
    pin: PrPin,
}

pub(crate) fn tool_definition() -> Value {
    tool(
        "task_link_pr",
        "Verify and pin an existing GitHub or GitHub Enterprise PR to a checkout in a Layer task. \
         This replaces any different pin on that checkout but does not change GitHub. \
         Returns the checkout ID and canonical pin. Call only after the user agrees.",
        json!({
            "task_id": str_prop("Task id from list_tasks"),
            "checkout_id": str_prop("Checkout id from task_prs for this task"),
            "pr_url": str_prop("Browser link to the existing pull request"),
            "confirm": confirm_prop()
        }),
        vec!["task_id", "checkout_id", "pr_url", "confirm"],
    )
}

pub(crate) async fn call_tool(app: AppHandle, args: Value) -> Result<Value> {
    let task_id = required(&args, "task_id")?.to_string();
    let checkout_id = required(&args, "checkout_id")?.to_string();
    let pr_url = required(&args, "pr_url")?.to_string();
    Ok(serde_json::to_value(link_task_pr_inner(app, task_id, checkout_id, pr_url).await?)?)
}

/// Forget a local association, even when GitHub is unavailable.
#[tauri::command]
pub async fn unpin_task_pr<R: tauri::Runtime>(
    app: AppHandle<R>,
    task_id: String,
    checkout_id: String,
    pin_revision: String,
) -> Result<()> {
    let owner = task_id.clone();
    let changed = super::blocking(app.clone(), move |state| {
        let _generation = state.config.tracker_credential_generation(TrackerKind::Github);
        let changed = state.config.update_local_tasks_if(|config| {
            let at = checkout_index(config, &owner, &checkout_id)?;
            let checkout = &mut config.checkouts[at];
            if checkout.pr_pin_revision.to_string() != pin_revision {
                return Err(Error::PrPinRevisionChanged);
            }
            let changed = checkout.replace_pr_pin(None)?;
            Ok((changed, changed))
        })?;
        if changed {
            super::pr_resolver::invalidate(state, &owner);
        }
        Ok(changed)
    })
    .await?;
    notify_pin_change(&app, &task_id, changed);
    Ok(())
}

fn notify_pin_change<R: tauri::Runtime>(app: &AppHandle<R>, task_id: &str, changed: bool) {
    if changed {
        let _ = app.emit("task:pr-pin-changed", task_id);
    }
}

async fn link_task_pr_inner(app: AppHandle, task_id: String, checkout_id: String, pr_url: String) -> Result<LinkedPr> {
    link_task_pr_with(app, task_id, checkout_id, pr_url, BoundGithub::load).await
}

async fn link_task_pr_with<R: tauri::Runtime>(
    app: AppHandle<R>,
    task_id: String,
    checkout_id: String,
    pr_url: String,
    connect: impl FnOnce(&AppState) -> Result<BoundGithub> + Send + 'static,
) -> Result<LinkedPr> {
    let owner = task_id.clone();
    let (connection, checkout, repo, requested, web) = super::blocking(app.clone(), move |state| {
        let checkout =
            state.config.with(|c| Ok::<_, Error>(c.checkouts[checkout_index(c, &task_id, &checkout_id)?].clone()))?;
        let connection = connect(state)?;
        let web = configured_web(connection.config())?;
        let repo = configured_checkout_repo(Path::new(&checkout.path), connection.config())?;
        let slug = format!("{}/{}", repo.0, repo.1);
        let number = pr_number(&web, &slug, pr_url.trim())?;
        let requested = StoredPrPin { pin: canonical_pin(&web, slug, number), identity: None };
        Ok((connection, checkout, repo, requested, web))
    })
    .await?;
    let (verified, _) = read_pin(&connection, (&repo.0, &repo.1), &requested).await?;

    let (result, changed) = super::blocking(app.clone(), move |state| {
        // Git's origin can be edited outside Layer. Re-read it after the
        // network wait, without holding the config or credential lock.
        let current_repo = configured_checkout_repo(Path::new(&checkout.path), connection.config())?;
        if !current_repo.0.eq_ignore_ascii_case(&repo.0) || !current_repo.1.eq_ignore_ascii_case(&repo.1) {
            return Err(Error::Other("The checkout's repository changed during lookup. Try again.".into()));
        }
        let generation = state.config.tracker_credential_generation(TrackerKind::Github);
        let (result, changed) = state.config.update_local_tasks_if(|config| {
            if !connection.is_current(config, *generation) {
                return Err(Error::Other("The GitHub connection changed during lookup. Try again.".into()));
            }
            let at = checkout_index(config, &checkout.task_id, &checkout.id)?;
            let current = &config.checkouts[at];
            if current.path != checkout.path || current.project_id != checkout.project_id {
                return Err(Error::Other("The checkout changed during lookup. Try again.".into()));
            }
            // Continue for an identical concurrent result, but do not let a
            // delayed replacement overwrite a different, newly saved pin.
            if current.pr_pin_revision != checkout.pr_pin_revision && current.pr_pin.as_ref() != Some(&verified) {
                return Err(Error::Other("The checkout's pin changed during lookup. Check it and try again.".into()));
            }
            if config.checkouts.iter().any(|other| {
                other.id != checkout.id
                    && other.pr_pin.as_ref().is_some_and(|other| same_pin_identity(&web, other, &verified))
            }) {
                return Err(Error::Other("This pull request is already pinned to another checkout.".into()));
            }
            let pin = verified.pin.clone();
            let changed = config.checkouts[at].replace_pr_pin(Some(verified))?;
            Ok(((LinkedPr { checkout_id: checkout.id, pin }, changed), changed))
        })?;
        if changed {
            super::pr_resolver::invalidate(state, &checkout.task_id);
        }
        Ok((result, changed))
    })
    .await?;
    notify_pin_change(&app, &owner, changed);
    Ok(result)
}

fn checkout_index(config: &AppConfig, task_id: &str, checkout_id: &str) -> Result<usize> {
    let task = config.tasks.iter().find(|task| task.id == task_id).ok_or_else(|| Error::NotFound("Task".into()))?;
    if task.review.is_some() {
        return Err(Error::Other("A legacy review task cannot pin a pull request.".into()));
    }
    config
        .checkouts
        .iter()
        .position(|checkout| checkout.task_id == task_id && checkout.id == checkout_id)
        .ok_or_else(|| Error::NotFound("The checkout is not in this task.".into()))
}

fn same_pin_identity(web: &Url, other: &StoredPrPin, verified: &StoredPrPin) -> bool {
    if let Some(identity) = &other.identity {
        return verified.identity.as_ref() == Some(identity);
    }
    // Older pins can only prove duplicate ownership by site, slug and number.
    other.pin.number == verified.pin.number && checked_pin_url(web, &verified.pin.repo, &other.pin).is_ok()
}

/// Use the same connection snapshot for the client and for the repo derived
/// by `configured_checkout_repo`. A stored link proves no remote state.
pub(crate) async fn read_pin(
    connection: &BoundGithub,
    (owner, repo): (&str, &str),
    saved: &StoredPrPin,
) -> Result<(StoredPrPin, PullRequest)> {
    let web = configured_web(connection.config())?;
    let slug = format!("{owner}/{repo}");
    let pin = &saved.pin;
    connection.checked_pin_url(&slug, saved)?;
    let expected_id = saved.identity.as_ref().map(|identity| identity.id);
    let (mut pr, canonical_repo, id) =
        connection.client().pin_pull(connection.config(), owner, repo, pin.number, expected_id).await?;
    if !matches!(pr_number(&web, &slug, &pr.url), Ok(n) if n == pin.number) {
        return Err(Error::Other("GitHub did not return a matching pull request link. Check the connection.".into()));
    }
    let pin = canonical_pin(&web, canonical_repo, pr.number);
    let identity = Some(PrPinIdentity { id, api_origin: connection.api_origin.clone() });
    pr.url.clone_from(&pin.url);
    let verified = StoredPrPin { pin, identity };
    Ok((verified, pr))
}

pub(super) fn display_pin(config: &GithubConfig, repo: &str, saved: &StoredPrPin) -> Result<String> {
    let origin = api_origin(config)?;
    if saved.identity.as_ref().is_some_and(|identity| identity.api_origin != origin) {
        return Err(Error::PrPinIdentityChanged);
    }
    checked_pin_url(&configured_web(config)?, repo, &saved.pin)
}

pub(super) fn api_origin(connection: &GithubConfig) -> Result<String> {
    Ok(configured_base(&connection.api_url)?.origin().ascii_serialization())
}

pub(crate) fn configured_web(connection: &GithubConfig) -> Result<Url> {
    configured_base(&connection.web_url)
}

pub(super) fn validated_pr_link(config: &GithubConfig, slug: &str, pr: &PullRequest) -> Result<String> {
    let web = configured_web(config)?;
    if pr_number(&web, slug, &pr.url)? != pr.number {
        return Err(Error::Other("GitHub did not return a matching pull request link. Check the connection.".into()));
    }
    Ok(canonical_url(&web, slug, pr.number))
}

fn configured_base(raw: &str) -> Result<Url> {
    strict_url(raw)
        .filter(|url| url.query().is_none() && url.fragment().is_none())
        .ok_or_else(|| Error::Other("The configured GitHub addresses are not valid. Check the connection.".into()))
}

/// Safe display link for locally valid pin data even if the PR lookup fails.
/// Do not hand a raw config URL to the browser.
fn checked_pin_url(web: &Url, repo: &str, pin: &PrPin) -> Result<String> {
    if !pin.repo.eq_ignore_ascii_case(repo) || !matches!(pr_number(web, repo, &pin.url), Ok(n) if n == pin.number) {
        return Err(Error::Other("The saved pin does not match this checkout and GitHub connection.".into()));
    }
    Ok(canonical_url(web, &pin.repo, pin.number))
}

fn canonical_url(web: &Url, repo: &str, number: u64) -> String {
    format!("{}/{repo}/pull/{number}", web.as_str().trim_end_matches('/'))
}

fn canonical_pin(web: &Url, repo: String, number: u64) -> PrPin {
    PrPin { number, url: canonical_url(web, &repo, number), repo }
}

fn pr_number(web: &Url, expected_repo: &str, raw: &str) -> Result<u64> {
    let invalid = || Error::Other("Use a pull request link for this checkout on the configured GitHub site.".into());
    let url = strict_url(raw).filter(|url| url.origin() == web.origin()).ok_or_else(invalid)?;
    let route = url
        .path()
        .strip_prefix(web.path().trim_end_matches('/'))
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(invalid)?;
    let mut parts = route.strip_suffix('/').unwrap_or(route).split('/');
    let (Some(owner), Some(repo), Some("pull"), Some(number), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    if ![owner, repo].into_iter().all(valid_repo_segment) || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    if !format!("{owner}/{repo}").eq_ignore_ascii_case(expected_repo) {
        return Err(Error::Other("The pull request link points to another repository.".into()));
    }
    number.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(invalid)
}

fn strict_url(raw: &str) -> Option<Url> {
    // Reject userinfo even if empty, and any route the URL parser silently
    // rewrites (such as an encoded dot segment or a backslash separator).
    if raw.contains('\\') || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (_, after_scheme) = raw.split_once("://")?;
    let at = after_scheme.find(['/', '?', '#']).unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(at);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let path = rest.split(['?', '#']).next()?;
    let url = Url::parse(raw).ok()?;
    let path = if path.is_empty() { "/" } else { path };
    (matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == path)
        .then_some(url)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::{Json, Router};
    use tauri::test::{mock_app, MockRuntime};
    use tauri::Manager;
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    use super::super::repos::tests::{add_task, git, setup, state};
    use super::*;

    const TOKEN: &str = "fixture-token-do-not-copy";

    struct Fixture {
        app: tauri::App<MockRuntime>,
        root: PathBuf,
        listener: Option<TcpListener>,
        site: String,
        credential_origin: Option<String>,
        requests: Arc<Mutex<Vec<(Method, String)>>>,
    }

    impl Fixture {
        async fn new() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let site = format!("http://{address}");
            let (root, mut config) = setup();
            config.github = Some(GithubConfig {
                api_url: format!("{site}/api/v3"),
                web_url: format!("{site}/code/"),
                ..Default::default()
            });
            let repo = PathBuf::from(&config.projects[0].path);
            git(&repo, &["remote", "set-url", "origin", &format!("{site}/acme/api.git")]);
            for name in ["first", "second"] {
                add_task(&mut config, &root, name);
                let checkout = config.checkouts.last().unwrap();
                std::fs::create_dir_all(Path::new(&checkout.path).parent().unwrap()).unwrap();
                git(&repo, &["worktree", "add", "-qb", name, &checkout.path]);
            }
            let app = mock_app();
            app.manage(state(&root, config));
            Self {
                app,
                root,
                listener: Some(listener),
                credential_origin: Some(site.clone()),
                site,
                requests: Arc::default(),
            }
        }

        fn serve<F, Fut>(&mut self, reply: F)
        where
            F: Fn(u64) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Response> + Send,
        {
            let requests = self.requests.clone();
            let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
                let requests = requests.clone();
                let reply = reply.clone();
                async move {
                    requests.lock().unwrap().push((method.clone(), uri.to_string()));
                    if method == Method::GET
                        && headers
                            .get("authorization")
                            .and_then(|header| header.to_str().ok())
                            .is_some_and(|header| header == format!("Bearer {TOKEN}").as_str())
                    {
                        if let Some(number) = uri
                            .path()
                            .strip_prefix("/api/v3/repos/acme/api/pulls/")
                            .or_else(|| uri.path().strip_prefix("/api/v3/repos/acme/renamed/pulls/"))
                            .and_then(|n| n.parse().ok())
                        {
                            return reply(number).await;
                        }
                    }
                    StatusCode::BAD_REQUEST.into_response()
                }
            });
            let listener = self.listener.take().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        }

        fn serve_prs(&mut self) {
            let site = self.site.clone();
            self.serve(move |number| {
                let pr = response(&site, number);
                async move { Json(pr).into_response() }
            });
        }

        fn state(&self) -> tauri::State<'_, super::super::AppState> {
            self.app.state()
        }

        fn pin_url(&self, number: u64) -> String {
            format!("{}/code/acme/api/pull/{number}", self.site)
        }

        fn saved(&self) -> AppConfig {
            let on_disk = std::fs::read(self.state().config.folder().join("config.json")).unwrap();
            assert!(!String::from_utf8_lossy(&on_disk).contains(TOKEN));
            serde_json::from_slice(&on_disk).unwrap()
        }

        async fn link(&self, task: &str, checkout: &str, url: &str) -> Result<Value> {
            let origin = self.credential_origin.clone();
            let linked =
                link_task_pr_with(self.app.handle().clone(), task.into(), checkout.into(), url.into(), move |state| {
                    BoundGithub::load_with(state, || Ok(secrets::GithubCredential::fixture(TOKEN, origin.as_deref())))
                })
                .await?;
            Ok(serde_json::to_value(linked)?)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn response(site: &str, number: u64) -> Value {
        json!({
            "id": 10000 + number, "number": number, "title": "Pin a PR", "state": "open", "draft": false,
            "user": {"login": "dev"},
            "head": {"repo": {"full_name": "Dev/Fork"}, "ref": "topic", "sha": "1111111111111111111111111111111111111111"},
            "base": {"repo": {"full_name": "Acme/Api"}, "ref": "main"},
            "html_url": format!("{site}/code/Acme/Api/pull/{number}"),
            "merged": false, "merged_at": null, "comments": 2, "review_comments": 3,
            "mergeable_state": "clean"
        })
    }

    #[tokio::test]
    async fn only_a_durable_pin_change_notifies_the_window() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let config_path = f.state().config.folder().join("config.json");
        tauri::Listener::listen(&f.app, "task:pr-pin-changed", move |event| {
            let cfg: AppConfig = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
            tx.send((serde_json::from_str::<String>(event.payload()).unwrap(), cfg.checkouts[0].pr_pin_revision))
                .unwrap();
        });
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        assert_eq!(rx.try_recv().unwrap(), ("first".into(), 1));
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        assert!(rx.try_recv().is_err(), "a repeat must not request another read");

        let state = f.state();
        let began = std::time::Instant::now();
        state.prs_settled.lock().insert("first".into(), began);
        state.prs_seen.lock().insert("first".into(), Vec::new());
        assert!(unpin_task_pr(f.app.handle().clone(), "second".into(), "first-api".into(), "1".into()).await.is_err());
        assert!(rx.try_recv().is_err());
        assert!(state.prs_settled.lock().contains_key("first"));

        unpin_task_pr(f.app.handle().clone(), "first".into(), "first-api".into(), "1".into()).await.unwrap();
        assert_eq!(rx.try_recv().unwrap(), ("first".into(), 2));
        assert!(f.saved().checkouts[0].pr_pin.is_none());
        assert!(!state.prs_settled.lock().contains_key("first"));
        assert!(!state.prs_seen.lock().contains_key("first"));
        let invalidated = state.prs_read_at.lock()["first"];
        assert!(invalidated >= began);

        unpin_task_pr(f.app.handle().clone(), "first".into(), "first-api".into(), "2".into()).await.unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(f.saved().checkouts[0].pr_pin_revision, 2);
        assert_eq!(state.prs_read_at.lock()["first"], invalidated);
        assert_eq!(f.requests.lock().unwrap().len(), 2, "unpin must never call GitHub");
    }

    #[tokio::test]
    async fn a_failed_unpin_save_does_not_clear_the_pin_or_emit_a_refresh() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        let state = f.state();
        let path = state.config.folder().join("config.json");
        std::fs::rename(&path, path.with_extension("backup")).unwrap();
        std::fs::create_dir(&path).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tauri::Listener::listen(&f.app, "task:pr-pin-changed", move |_| tx.send(()).unwrap());
        let before = state.prs_read_at.lock()["first"];

        assert!(unpin_task_pr(f.app.handle().clone(), "first".into(), "first-api".into(), "1".into()).await.is_err());
        assert!(rx.try_recv().is_err());
        let checkout = state.config.checkout("first-api").unwrap();
        assert_eq!(checkout.pr_pin.unwrap().pin.number, 42);
        assert_eq!(checkout.pr_pin_revision, 1);
        assert_eq!(state.prs_read_at.lock()["first"], before);
    }

    #[tokio::test]
    async fn step7_round_one_unpin_cannot_remove_a_replacement_after_aba() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        let remove_displayed = || unpin_task_pr(f.app.handle().clone(), "first".into(), "first-api".into(), "1".into());
        f.link("first", "first-api", &f.pin_url(43)).await.unwrap();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();

        assert!(matches!(remove_displayed().await, Err(Error::PrPinRevisionChanged)));
        let checkout = &f.saved().checkouts[0];
        assert_eq!(checkout.pr_pin.as_ref().unwrap().pin.number, 42);
        assert_eq!(checkout.pr_pin_revision, 3);
        let state = f.state();
        state.config.update(|c| c.checkouts[0].pr_pin_revision = 9_007_199_254_740_993).unwrap();
        let app = f.app.handle();
        assert!(matches!(
            unpin_task_pr(app.clone(), "first".into(), "first-api".into(), "9007199254740992".into()).await,
            Err(Error::PrPinRevisionChanged)
        ));
        let rows = super::super::github::github_task_prs_from(
            &state,
            "first",
            super::super::pr_resolver::Connection::load_with(&state, || {
                Err(Error::Other("fixture credential store failure".into()))
            }),
        )
        .await
        .unwrap();
        assert_eq!(serde_json::to_value(&rows[0]).unwrap()["pin_revision"], "9007199254740993");
        unpin_task_pr(app.clone(), "first".into(), "first-api".into(), rows[0].pin_revision.clone()).await.unwrap();
        assert_eq!(f.saved().checkouts[0].pr_pin_revision, 9_007_199_254_740_994);
    }

    #[tokio::test]
    async fn a_disconnected_task_still_lists_its_pin_without_exposing_the_saved_url() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        f.state().config.update(|c| c.github = None).unwrap();
        let rows = super::super::github_task_prs(f.state(), "first".into()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].checkout_id, "first-api");
        let pin = rows[0].pin.as_ref().unwrap();
        assert_eq!(pin.number, 42);
        assert!(pin.url.is_empty());
        assert!(!rows[0].pin_read);
        assert!(rows[0].error.is_some());
        let unpinned = super::super::github_task_prs(f.state(), "second".into()).await.unwrap();
        assert_eq!(unpinned.len(), 1);
        assert!(unpinned[0].pin.is_none());
        assert!(unpinned[0].error.is_some(), "not connected is not an empty PR read");
        unpin_task_pr(f.app.handle().clone(), "first".into(), "first-api".into(), "1".into()).await.unwrap();
        let removed = super::super::github_task_prs(f.state(), "first".into()).await.unwrap();
        assert!(removed[0].pin.is_none());
        assert!(removed[0].error.is_some());
        assert_eq!(f.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_verified_pr_pin_survives_reload_and_repeating_it_keeps_one_pin() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let raw = format!("{}/code/acme/api/pull/042?token={TOKEN}#files", f.site);
        let pin = json!({"number": 42, "repo": "Acme/Api", "url": format!("{}/code/Acme/Api/pull/42", f.site)});
        let before = serde_json::to_value(f.state().config.read()).unwrap();
        let linked = f.link("first", "first-api", &raw).await.unwrap();
        assert_eq!(linked, json!({"checkout_id": "first-api", "pin": pin}));
        let mut after = before;
        after["checkouts"][0]["pr_pin"] = pin;
        after["checkouts"][0]["pr_pin"]["identity"] = json!({"api_origin": f.site, "id": 10042});
        after["checkouts"][0]["pr_pin_revision"] = 1.into();
        assert_eq!(serde_json::to_value(f.saved()).unwrap(), after);
        assert_eq!(f.link("first", "first-api", &f.pin_url(42)).await.unwrap(), linked);
        assert_eq!(serde_json::to_value(f.saved()).unwrap(), after);
        assert_eq!(*f.requests.lock().unwrap(), vec![(Method::GET, "/api/v3/repos/acme/api/pulls/42".into()); 2]);
    }

    #[tokio::test]
    async fn invalid_pr_addresses_and_wrong_checkout_ownership_never_reach_the_api() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let ok = f.pin_url(42);
        for url in [
            ok.replace("127.0.0.1", "localhost"),
            ok.replace("http:", "https:"),
            "http://127.0.0.1:1/code/acme/api/pull/42".into(),
            ok.replace("://", &format!("://user:{TOKEN}@")),
            ok.replace("://", "://@"),
            ok.replace("/code/", "/code2/"),
            ok.replace("/code/", "/code/ignored/../"),
            ok.replace("/code/", "/code/ignored/%2E%2E/"),
            ok.replace("/code/", "/code\\"),
            ok.replace("/acme/", "/other/"),
            ok.replace("/api/", "/other/"),
            ok.replace("/pull/", "/issues/"),
            ok.replace("/42", "/0"),
            ok.replace("/42", "/+42"),
            ok.replace("/42", "/18446744073709551616"),
            format!("{ok}/files"),
        ] {
            let error = f.link("first", "first-api", &url).await.unwrap_err();
            assert!(!error.to_string().contains(TOKEN));
        }
        assert!(f.link("second", "first-api", &ok).await.is_err());
        assert!(f.requests.lock().unwrap().is_empty());
        assert!(!f.state().config.folder().join("config.json").exists());
    }

    #[tokio::test]
    async fn a_legacy_review_or_checkout_on_another_forge_is_refused_before_lookup() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let url = f.pin_url(42);
        let mut config = f.state().config.read();
        let path = Path::new(&config.checkouts[0].path);
        let different_origin = format!("{}/acme/api.git", f.site.replace("127.0.0.1", "localhost"));
        git(path, &["remote", "set-url", "origin", &different_origin]);
        assert!(f.link("first", "first-api", &url).await.unwrap_err().to_string().contains("origin"));
        git(path, &["remote", "set-url", "origin", &format!("{}/acme/api.git", f.site)]);
        config.tasks[0].review = Some(crate::config::ReviewOf {
            repo: "Acme/Api".into(),
            number: 42,
            url: url.clone(),
            author: "dev".into(),
            head_sha: "1111111111111111111111111111111111111111".into(),
        });
        f.state().config.update(|c| *c = config).unwrap();
        assert!(f.link("first", "first-api", &url).await.unwrap_err().to_string().contains("legacy review task"));
        assert!(f.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_wrong_or_failed_api_response_cannot_create_a_pin_or_expose_credentials() {
        let mut f = Fixture::new().await;
        let site = f.site.clone();
        f.serve(move |number| {
            let mut pr = response(&site, number);
            async move {
                match number {
                    41 => pr["base"]["repo"]["full_name"] = json!("Other/Api"),
                    42 => pr["number"] = json!(100),
                    43 => pr["html_url"] = json!(format!("https://other.test/Acme/Api/pull/{number}")),
                    44 => return (StatusCode::BAD_GATEWAY, format!("proxy echoed {TOKEN}")).into_response(),
                    45 => {
                        return (StatusCode::FOUND, [("location", "/api/v3/repos/acme/api/pulls/46")]).into_response()
                    }
                    47 => pr["merged"] = Value::Null,
                    48 => pr["id"] = json!(0),
                    _ => {}
                }
                Json(pr).into_response()
            }
        });
        for number in [41, 42, 43, 44, 45, 47, 48] {
            let error = f.link("first", "first-api", &f.pin_url(number)).await.unwrap_err();
            assert!(!error.to_string().contains(TOKEN));
        }
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 7, "the redirect must not generate a second request");
        assert!(requests.iter().all(|(method, _)| method == Method::GET));
        assert!(!f.state().config.folder().join("config.json").exists());
    }

    #[tokio::test]
    async fn a_pr_claimed_by_another_checkout_cannot_be_pinned_twice() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        f.link("second", "second-api", &f.pin_url(43)).await.unwrap();
        let error = f.link("second", "second-api", &f.pin_url(42)).await.unwrap_err();
        assert!(error.to_string().contains("already pinned"));
        let saved = serde_json::to_value(f.saved()).unwrap();
        assert_eq!(saved["checkouts"][0]["pr_pin"]["number"], 42);
        assert_eq!(saved["checkouts"][1]["pr_pin"]["number"], 43);
        f.link("second", "second-api", &f.pin_url(44)).await.unwrap();
        assert_eq!(serde_json::to_value(f.saved()).unwrap()["checkouts"][1]["pr_pin"]["number"], 44);
    }

    #[tokio::test]
    async fn a_failed_disk_write_neither_exposes_nor_reserves_the_pr_pin() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let path = f.state().config.folder().join("config.json");
        std::fs::create_dir(&path).unwrap();
        let url = f.pin_url(42);
        assert!(matches!(f.link("first", "first-api", &url).await, Err(Error::Io(_))));
        assert!(f.state().config.checkout("first-api").unwrap().pr_pin.is_none());
        std::fs::remove_dir(&path).unwrap();
        assert!(f.link("second", "second-api", &url).await.is_ok());
    }

    #[tokio::test]
    async fn a_saved_pin_resolves_after_switching_branches_but_never_on_unchecked_data() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        let config = f.saved();
        let checkout = &config.checkouts[0];
        let pin = checkout.pr_pin.as_ref().unwrap();
        let path = Path::new(&checkout.path);
        git(path, &["checkout", "-qb", "new-topic"]);
        let connection = BoundGithub::load_with(&f.state(), || {
            Ok(secrets::GithubCredential::fixture(TOKEN, f.credential_origin.as_deref()))
        })
        .unwrap();
        let (owner, repo) = configured_checkout_repo(path, connection.config()).unwrap();
        for bad_link in [
            PrPin { url: "javascript:bad".into(), ..pin.pin.clone() },
            PrPin { repo: "Other/Api".into(), ..pin.pin.clone() },
            PrPin { number: 41, ..pin.pin.clone() },
            PrPin { url: pin.pin.url.replace("127.0.0.1", "localhost"), ..pin.pin.clone() },
        ] {
            let bad = StoredPrPin { pin: bad_link, ..pin.clone() };
            assert!(read_pin(&connection, (&owner, &repo), &bad).await.is_err());
        }
        let mut wrong_forge = pin.clone();
        wrong_forge.identity.as_mut().unwrap().api_origin = "https://elsewhere.test".into();
        assert!(read_pin(&connection, (&owner, &repo), &wrong_forge).await.is_err());
        assert_eq!(f.requests.lock().unwrap().len(), 1, "invalid saved pins must fail before looking up any PR");
        let (verified, pr) = read_pin(&connection, (&owner, &repo), pin).await.unwrap();
        assert_eq!(&verified, pin);
        assert_eq!(pr.head, "topic");
        assert_eq!(pr.comments, 2);
        assert_eq!(f.requests.lock().unwrap().len(), 2);
        let mut wrong_id = pin.clone();
        wrong_id.identity.as_mut().unwrap().id = std::num::NonZeroU64::new(10041).unwrap();
        assert!(read_pin(&connection, (&owner, &repo), &wrong_id).await.is_err());
    }

    #[tokio::test]
    async fn a_delayed_pr_lookup_cannot_replace_a_pin_saved_while_it_waited() {
        let mut f = Fixture::new().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let site = f.site.clone();
        let waits = (started.clone(), release.clone());
        f.serve(move |number| {
            let (started, release) = waits.clone();
            let pr = response(&site, number);
            async move {
                if number == 42 {
                    started.notify_one();
                    release.notified().await;
                }
                Json(pr).into_response()
            }
        });
        let old = f.pin_url(42);
        let new = f.pin_url(43);
        let (late, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(f.link("first", "first-api", &old), async {
                started.notified().await;
                f.link("first", "first-api", &new).await.unwrap();
                release.notify_one();
            })
        })
        .await
        .unwrap();
        assert!(late.unwrap_err().to_string().contains("pin changed"));
        assert_eq!(serde_json::to_value(f.saved()).unwrap()["checkouts"][0]["pr_pin"]["number"], 43);
    }

    #[tokio::test]
    async fn a_lookup_cannot_save_after_the_connection_or_checkout_changes() {
        for change in ["token", "endpoints", "membership", "origin"] {
            let mut f = Fixture::new().await;
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let site = f.site.clone();
            let waits = (started.clone(), release.clone());
            f.serve(move |number| {
                let (started, release) = waits.clone();
                let pr = response(&site, number);
                async move {
                    started.notify_one();
                    release.notified().await;
                    Json(pr).into_response()
                }
            });
            let url = f.pin_url(42);
            let (late, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(f.link("first", "first-api", &url), async {
                    started.notified().await;
                    let state = f.state();
                    match change {
                        "token" => *state.config.tracker_credential_generation(TrackerKind::Github) += 1,
                        "endpoints" => {
                            state.config.update(|c| c.github.as_mut().unwrap().web_url.push_str("/changed")).unwrap()
                        }
                        "membership" => state.config.update(|c| c.checkouts[0].task_id = "second".into()).unwrap(),
                        "origin" => {
                            let checkout = state.config.checkout("first-api").unwrap();
                            git(
                                Path::new(&checkout.path),
                                &["remote", "set-url", "origin", &format!("{}/other/api.git", f.site)],
                            );
                        }
                        _ => panic!("unknown test change: {change}"),
                    }
                    release.notify_one();
                })
            })
            .await
            .unwrap();
            assert!(late.is_err(), "saved the pin after changing {change}");
            assert!(f.state().config.checkout("first-api").unwrap().pr_pin.is_none());
        }
    }

    #[tokio::test]
    async fn review_one_workspace_endpoint_changes_cannot_receive_the_saved_token() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let new = f.state().config.read().github.unwrap();
        let old_site = "http://127.0.0.1:1";
        f.credential_origin = Some(old_site.into());
        f.state()
            .config
            .update(|c| {
                let gh = c.github.as_mut().unwrap();
                gh.api_url = format!("{old_site}/api/v3");
                gh.web_url = old_site.into();
            })
            .unwrap();
        f.state()
            .config
            .mirror(|c| {
                crate::config::shared::take_held(
                    c,
                    "github",
                    &json!({"api_url": new.api_url, "web_url": new.web_url, "credential_origin": f.site}),
                    true,
                )
            })
            .unwrap();
        let answer = f.link("first", "first-api", &f.pin_url(42)).await;
        assert!(f.requests.lock().unwrap().is_empty(), "the synced origin received the same saved token");
        assert!(answer.err().unwrap().to_string().contains("Reconnect GitHub"));
        f.credential_origin = None;
        let legacy = f.link("first", "first-api", &f.pin_url(42)).await.unwrap_err();
        assert!(legacy.to_string().contains("Reconnect GitHub"));
        assert!(f.requests.lock().unwrap().is_empty(), "a legacy token without a bound origin was sent");
    }

    #[tokio::test]
    async fn review_one_a_renamed_repo_does_not_let_another_checkout_claim_the_same_pr_id() {
        let mut f = Fixture::new().await;
        let renamed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reply_renamed = renamed.clone();
        let site = f.site.clone();
        f.serve(move |number| {
            let mut pr = response(&site, number);
            if reply_renamed.load(std::sync::atomic::Ordering::SeqCst) {
                pr["base"]["repo"]["full_name"] = json!("Acme/Renamed");
                pr["html_url"] = json!(format!("{site}/code/Acme/Renamed/pull/{number}"));
            }
            async move { Json(pr).into_response() }
        });
        f.link("first", "first-api", &f.pin_url(42)).await.unwrap();
        let checkout = f.state().config.checkout("second-api").unwrap();
        git(Path::new(&checkout.path), &["remote", "set-url", "origin", &format!("{}/acme/renamed.git", f.site)]);
        renamed.store(true, std::sync::atomic::Ordering::SeqCst);
        let url = format!("{}/code/acme/renamed/pull/42", f.site);
        let error = f.link("second", "second-api", &url).await.unwrap_err();
        assert!(error.to_string().contains("already pinned"));
        assert!(f.saved().checkouts[1].pr_pin.is_none());
    }

    #[tokio::test]
    async fn review_one_restoring_the_old_pin_does_not_authorize_a_stale_replacement() {
        let mut f = Fixture::new().await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let site = f.site.clone();
        let waits = (started.clone(), release.clone());
        f.serve(move |number| {
            let (started, release) = waits.clone();
            let pr = response(&site, number);
            async move {
                if number == 44 {
                    started.notify_one();
                    release.notified().await;
                }
                Json(pr).into_response()
            }
        });
        let a = f.pin_url(42);
        f.link("first", "first-api", &a).await.unwrap();
        let pending = f.pin_url(44);
        let (late, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(f.link("first", "first-api", &pending), async {
                started.notified().await;
                f.link("first", "first-api", &f.pin_url(43)).await.unwrap();
                f.link("first", "first-api", &a).await.unwrap();
                release.notify_one();
            })
        })
        .await
        .unwrap();
        assert!(late.unwrap_err().to_string().contains("pin changed"));
        assert_eq!(serde_json::to_value(f.saved()).unwrap()["checkouts"][0]["pr_pin"]["number"], 42);
    }

    #[tokio::test]
    async fn review_one_classifies_body_only_sso_without_returning_the_body() {
        let mut f = Fixture::new().await;
        f.serve(|number| async move {
            let prefix = if number == 43 { "x".repeat(8192) } else { String::new() };
            (StatusCode::FORBIDDEN, format!("{prefix}SAML enforcement. Proxy body included {TOKEN}.")).into_response()
        });
        let sso = f.link("first", "first-api", &f.pin_url(42)).await.unwrap_err();
        assert!(matches!(sso, Error::TokenNeedsSso("GitHub")), "unexpected classification: {sso}");
        let beyond_limit = f.link("first", "first-api", &f.pin_url(43)).await.unwrap_err();
        assert!(matches!(beyond_limit, Error::Other(_)));
        assert!(!beyond_limit.to_string().contains(TOKEN));
    }

    #[tokio::test]
    async fn review_two_an_older_builds_replacement_token_never_reaches_the_pin_api() {
        let mut f = Fixture::new().await;
        f.serve_prs();
        let origin = f.site.clone();
        let answer = link_task_pr_with(
            f.app.handle().clone(),
            "first".into(),
            "first-api".into(),
            f.pin_url(42),
            move |state| {
                BoundGithub::load_with(state, || {
                    Ok(secrets::GithubCredential::fixture_after_older_rewrite(TOKEN, &origin, "replacement-token"))
                })
            },
        )
        .await;
        assert!(f.requests.lock().unwrap().is_empty(), "the replacement token was sent to the old API origin");
        let error = answer.err().unwrap().to_string();
        assert!(error.contains("Reconnect GitHub"));
        assert!(!error.contains("replacement-token"));
    }
}
