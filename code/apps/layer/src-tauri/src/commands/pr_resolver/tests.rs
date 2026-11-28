#[cfg(test)]
mod scenarios {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::IntoResponse;
    use axum::{Json, Router};
    use serde_json::{json, Value};

    use crate::config::{PrPin, StoredPrPin};
    use crate::secrets::GithubCredential;

    use super::super::super::finish_tool::validate_finish;
    use super::super::super::github::github_task_prs_from;
    use super::super::super::repos::tests::{add_task, commit, git, setup, state};
    use super::super::*;

    const TOKEN: &str = "fixture-token";

    struct Fixture {
        root: PathBuf,
        worktree: PathBuf,
        state: AppState,
        api: GithubConfig,
        site: String,
        listener: Mutex<Option<tokio::net::TcpListener>>,
    }

    impl Fixture {
        async fn new() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let site = format!("http://{}", listener.local_addr().unwrap());
            let api = GithubConfig { api_url: format!("{site}/api/v3"), web_url: site.clone(), ..Default::default() };
            let (root, mut cfg) = setup();
            let repo = PathBuf::from(&cfg.projects[0].path);
            git(&repo, &["remote", "set-url", "origin", &format!("{site}/acme/api.git")]);
            cfg.github = Some(api.clone());
            add_task(&mut cfg, &root, "task");
            let checkout = &mut cfg.checkouts[0];
            let worktree = PathBuf::from(&checkout.path);
            std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
            git(&repo, &["worktree", "add", "-qb", "task", &checkout.path]);
            let point = git(&worktree, &["rev-parse", "HEAD"]).trim().to_owned();
            git(&worktree, &["update-ref", "refs/remotes/origin/main", &point]);
            checkout.base_commit = Some(point);
            Self { state: state(&root, cfg), root, worktree, api, site, listener: Mutex::new(Some(listener)) }
        }

        fn connection(&self, bound: bool) -> Connection {
            let credential = GithubCredential::fixture(TOKEN, bound.then_some(self.site.as_str()));
            Connection::from_credential(self.api.clone(), 0, credential).unwrap()
        }

        fn pin(&self, number: u64) -> StoredPrPin {
            StoredPrPin {
                pin: PrPin { repo: "acme/api".into(), number, url: format!("{}/acme/api/pull/{number}", self.site) },
                identity: None,
            }
        }

        fn save_pin(&self, pin: Option<StoredPrPin>) {
            self.state.config.update_local_tasks(|c| c.checkouts[0].replace_pr_pin(pin)).unwrap();
        }

        fn scope(&self) -> Scope {
            Scope::capture(&self.state, "task", Instant::now()).unwrap()
        }

        fn item(&self, number: u64, branch: &str, sha: &str, head_repo: &str, merged: bool) -> Value {
            json!({
                "id": 1000 + number, "number": number, "title": "Change", "state": if merged {"closed"} else {"open"},
                "draft": false, "user": {"login": "dev"},
                "head": {"ref": branch, "sha": sha, "repo": {"full_name": head_repo}},
                "base": {"ref": "main", "repo": {"full_name": "acme/api"}},
                "html_url": format!("{}/acme/api/pull/{number}", self.site),
                "merged": merged, "merged_at": merged.then_some("2026-09-01T12:00:00Z"),
                "mergeable_state": "clean", "comments": 2, "review_comments": 1,
            })
        }

        fn serve(&self, task_pulls: Vec<Value>, detail: Option<Value>) -> Arc<Mutex<Vec<String>>> {
            self.serve_detail(task_pulls, detail.ok_or(StatusCode::NOT_FOUND))
        }

        fn serve_detail(
            &self,
            task_pulls: Vec<Value>,
            detail: std::result::Result<Value, StatusCode>,
        ) -> Arc<Mutex<Vec<String>>> {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = requests.clone();
            let listener = self.listener.lock().unwrap().take().unwrap();
            let router = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
                let (requests, task_pulls, detail) = (log.clone(), task_pulls.clone(), detail.clone());
                async move {
                    requests.lock().unwrap().push(uri.to_string());
                    assert_eq!(method, Method::GET, "the resolver must not write");
                    assert_eq!(headers["authorization"], format!("Bearer {TOKEN}"));
                    match uri.path() {
                        "/api/v3/repos/acme/api/pulls" => {
                            if uri.query().unwrap_or_default().contains("head=acme%3Atask&") {
                                Json(json!(task_pulls)).into_response()
                            } else {
                                Json(json!([])).into_response()
                            }
                        }
                        "/api/v3/repos/acme/api/pulls/42" => match detail {
                            Ok(pr) => Json(pr).into_response(),
                            Err(StatusCode::FORBIDDEN) => {
                                (StatusCode::FORBIDDEN, [("x-github-sso", "required; url=https://never-relay.example")])
                                    .into_response()
                            }
                            Err(status) => status.into_response(),
                        },
                        "/api/v3/repos/acme/right/pulls" => Json(json!([])).into_response(),
                        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
            });
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            requests
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn a_pin_survives_switching_head_and_an_open_pr_wins_over_a_merged_pin() {
        let f = Fixture::new().await;
        f.save_pin(Some(f.pin(42)));
        let initially = git::head_commit(&f.worktree).unwrap();
        git(&f.worktree, &["checkout", "-qb", "topic"]);
        let log = f.serve(
            vec![f.item(7, "task", &initially, "acme/api", false)],
            Some(f.item(42, "old-topic", &initially, "dev/fork", true)),
        );
        let result = f.scope().all(&f.connection(true), false).await.unwrap().pop().unwrap();
        assert_eq!(result.row.error, None);
        assert_eq!(result.row.pr.as_ref().unwrap().number, 7);
        assert_eq!(result.row.past[0].number, 42);
        assert_eq!(result.row.pin.as_ref().unwrap().number, 42);
        assert!(log.lock().unwrap().iter().any(|r| r == "/api/v3/repos/acme/api/pulls/42"));
        assert!(matches!(result.publication("task").unwrap(), Publication::Reuse(pr) if pr.number == 7));
        assert!(validate_finish(&[result], false).unwrap_err().to_string().contains("#7"));
    }

    #[tokio::test]
    async fn a_matching_branch_name_from_a_fork_does_not_reuse_the_task_head() {
        let f = Fixture::new().await;
        let sha = git::head_commit(&f.worktree).unwrap();
        f.save_pin(Some(f.pin(42)));
        // The same number occurs in both the list and pin detail.
        let fork = f.item(42, "task", &sha, "dev/api", false);
        f.serve(vec![fork.clone(), f.item(90, "task", &sha, "acme/api", false)], Some(fork));
        let result = f.scope().all(&f.connection(true), false).await.unwrap().pop().unwrap();
        assert_eq!(result.row.error, None);
        assert_eq!(result.row.past.iter().map(|p| p.number).collect::<Vec<_>>(), [90]);
        assert!(matches!(result.publication("task").unwrap(), Publication::Alternate(pr) if pr.number == 42));
    }

    #[tokio::test]
    async fn a_task_branch_at_base_with_only_uncommitted_changes_must_not_report_success() {
        let f = Fixture::new().await;
        std::fs::write(f.worktree.join("uncommitted"), "work").unwrap();
        f.serve(Vec::new(), None);
        let rows = f.scope().all(&f.connection(true), false).await.unwrap();
        assert!(rows[0].task_ref_landed);
        assert_eq!(rows[0].row.changed, 1);
        assert!(rows[0].nothing_to_publish().is_err());
    }

    #[tokio::test]
    async fn a_failed_pin_read_retains_the_automatic_result_but_blocks_publish_and_finish() {
        let f = Fixture::new().await;
        let sha = git::head_commit(&f.worktree).unwrap();
        f.save_pin(Some(f.pin(42)));
        let log = f.serve(vec![f.item(7, "task", &sha, "acme/api", true)], None);
        for bound in [false, true] {
            let rows = f.scope().all(&f.connection(bound), false).await.unwrap();
            let row = &rows[0];
            assert_eq!(row.row.pr.as_ref().unwrap().number, 7);
            assert_eq!(row.row.pin.as_ref().unwrap().url, f.pin(42).pin.url);
            assert!(row.row.error.as_deref().is_some_and(|e| e.contains(if bound {
                "404"
            } else {
                "Reconnect GitHub"
            })));
            assert!(row.publication("task").is_err());
            assert!(validate_finish(&rows, false).is_err());
            assert!(!super::super::super::github::task_merged(std::slice::from_ref(&row.row)));
            let n = log.lock().unwrap().iter().filter(|p| p.ends_with("/pulls/42")).count();
            assert_eq!(n, usize::from(bound), "only the bound connection may look up the numbered pin");
        }
    }

    #[tokio::test]
    async fn a_listed_pr_does_not_prove_the_same_numbered_pin_was_verified() {
        let f = Fixture::new().await;
        let sha = git::head_commit(&f.worktree).unwrap();
        f.save_pin(Some(f.pin(42)));
        f.serve(vec![f.item(42, "task", &sha, "acme/api", false)], None);
        let row = f.scope().all(&f.connection(true), false).await.unwrap().pop().unwrap().row;
        let shown = serde_json::to_value(row).unwrap();
        assert_eq!(shown["pr"]["number"], 42);
        assert_eq!(shown["pin"]["number"], 42);
        assert_eq!(shown["pin_read"], false);
        assert!(shown["error"].as_str().unwrap().contains("404"));
    }

    #[tokio::test]
    async fn step7_round_one_a_pin_with_a_contradicted_id_has_no_clickable_link() {
        let f = Fixture::new().await;
        let sha = git::head_commit(&f.worktree).unwrap();
        let mut pin = f.pin(42);
        pin.identity = Some(crate::config::PrPinIdentity {
            api_origin: f.site.clone(),
            id: std::num::NonZeroU64::new(7001).unwrap(),
        });
        f.save_pin(Some(pin));
        let same_path_different_id = f.item(42, "old-topic", &sha, "acme/api", false);
        f.serve(vec![f.item(7, "task", &sha, "acme/api", false)], Some(same_path_different_id));
        let row = f.scope().all(&f.connection(true), false).await.unwrap().pop().unwrap().row;
        assert_eq!(row.pr.unwrap().number, 7, "automatic results are still useful");
        let pin = row.pin.unwrap();
        assert_eq!(pin.number, 42);
        assert!(pin.url.is_empty(), "the numbered reply identified a different PR");
        assert!(!row.pin_read);
        assert!(row.error.unwrap().contains("no longer identifies"));
    }

    #[tokio::test]
    async fn step7_round_one_auth_failures_do_not_contradict_the_pin_or_lose_typed_guidance() {
        for (status, message) in [
            (StatusCode::UNAUTHORIZED, "GitHub no longer accepts the saved token"),
            (StatusCode::FORBIDDEN, "GitHub needs the saved token authorized for single sign-on"),
            (
                StatusCode::GATEWAY_TIMEOUT,
                "GitHub could not verify the request (504 Gateway Timeout). Check the connection.",
            ),
        ] {
            let f = Fixture::new().await;
            f.save_pin(Some(f.pin(42)));
            f.serve_detail(Vec::new(), Err(status));
            let row = f.scope().all(&f.connection(true), false).await.unwrap().pop().unwrap().row;
            assert_eq!(row.pin.unwrap().url, f.pin(42).pin.url);
            assert_eq!(row.error.as_deref(), Some(message));
            assert!(!row.pin_read);
        }
    }

    #[tokio::test]
    async fn step7_round_one_a_credential_read_failure_keeps_local_checkout_rows() {
        let f = Fixture::new().await;
        f.save_pin(Some(f.pin(42)));
        let message = "Could not read the GitHub connection. Try again.";
        let failed = || Err(Error::Other("fixture credential store failure".into()));
        let rows = github_task_prs_from(&f.state, "task", Connection::load_with(&f.state, failed)).await.unwrap();
        let row = &rows[0];
        assert_eq!(row.checkout_id, "task-api");
        assert_eq!(row.pin.as_ref().unwrap().number, 42);
        assert!(row.pin.as_ref().unwrap().url.is_empty());
        assert!(!row.pin_read && !row.branch_covered);
        assert_eq!(row.error.as_deref(), Some(message));
        let stale_failure = async {
            f.save_pin(Some(f.pin(43)));
            Connection::load_with(&f.state, failed).await
        };
        assert!(github_task_prs_from(&f.state, "task", stale_failure).await.is_err());
    }

    #[tokio::test]
    async fn a_proof_from_another_origin_never_falls_back_to_the_legacy_client() {
        let f = Fixture::new().await;
        f.save_pin(Some(f.pin(42)));
        let log = f.serve(Vec::new(), None);
        let credential = GithubCredential::fixture(TOKEN, Some("https://the-saved-site.example"));
        let connection = Connection::from_credential(f.api.clone(), 0, credential).unwrap();
        let rows = f.scope().all(&connection, false).await.unwrap();
        assert!(rows[0].row.error.as_deref().is_some_and(|e| e.contains("Reconnect GitHub")));
        assert_eq!(rows[0].row.pin.as_ref().unwrap().number, 42);
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_merged_pin_from_an_unrelated_topic_is_not_evidence_of_task_completion() {
        let f = Fixture::new().await;
        let head = git::head_commit(&f.worktree).unwrap();
        f.save_pin(Some(f.pin(42)));
        f.serve(Vec::new(), Some(f.item(42, "unrelated", &head, "dev/fork", true)));
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut evidence = scope.all(&connection, false).await.unwrap();
        assert!(validate_finish(&evidence, false).is_err());
        let rows = scope.accept_rows(&f.state, &connection, &mut evidence, true).await.unwrap();
        assert!(rows[0].pr.as_ref().unwrap().merged);
        assert!(rows[0].branch_covered);
        assert!(!rows[0].covered_merge);
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(head.as_str()));
    }

    #[tokio::test]
    async fn an_unreadable_head_does_not_discard_the_task_branch_or_pin_reads() {
        let f = Fixture::new().await;
        let old = git::head_commit(&f.worktree).unwrap();
        git(&f.worktree, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
        f.save_pin(Some(f.pin(42)));
        f.serve(
            vec![f.item(5, "task", &old, "acme/api", true)],
            Some(f.item(42, "old-topic", &old, "dev/fork", false)),
        );
        let rows = f.scope().all(&f.connection(true), false).await.unwrap();
        assert_eq!(rows[0].row.pr.as_ref().unwrap().number, 42);
        assert_eq!(rows[0].row.past[0].number, 5);
        assert!(rows[0].row.error.is_some());
        assert!(validate_finish(&rows, false).is_err());
    }

    #[tokio::test]
    async fn an_unrelated_merge_cannot_advance_or_finish_the_checkout_but_real_work_can() {
        let f = Fixture::new().await;
        let point = git::head_commit(&f.worktree).unwrap();
        f.save_pin(Some(f.pin(42)));
        commit(&f.worktree, "real-work");
        let work = git::head_commit(&f.worktree).unwrap();
        f.serve(
            vec![f.item(7, "task", &work, "acme/api", true)],
            Some(f.item(42, "unrelated", &point, "dev/fork", true)),
        );
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut rows = scope.all(&connection, false).await.unwrap();
        assert_eq!(rows[0].row.pr.as_ref().unwrap().number, 42, "pin wins among closed PRs");
        assert_eq!(rows[0].advance.as_deref(), Some(work.as_str()), "the older, relevant PR proves what landed");
        assert_eq!(rows[0].merged_heads, std::slice::from_ref(&work));
        assert!(validate_finish(&rows, false).is_ok());
        assert!(super::super::super::github::task_merged(
            &scope.accept_rows(&f.state, &connection, &mut rows, true).await.unwrap()
        ));
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(work.as_str()));
    }

    #[tokio::test]
    async fn changing_a_pin_back_to_its_original_value_still_supersedes_the_old_read() {
        let f = Fixture::new().await;
        f.save_pin(Some(f.pin(42)));
        f.serve(Vec::new(), None);
        let connection = f.connection(true);
        let mut stale_read = f.scope();
        let mut stale_rows = stale_read.all(&connection, false).await.unwrap();
        f.save_pin(Some(f.pin(43)));
        f.save_pin(Some(f.pin(42)));
        assert!(stale_read.accept_rows(&f.state, &connection, &mut stale_rows, true).await.is_err());
        assert!(!f.state.prs_seen.lock().contains_key("task"));
        assert!(!f.state.prs_settled.lock().contains_key("task"));
    }

    #[tokio::test]
    async fn a_slow_sweep_cannot_publish_after_a_newer_direct_read() {
        let f = Fixture::new().await;
        f.serve(Vec::new(), None);
        let connection = f.connection(true);
        let mut old = f.scope();
        let mut rows = old.all(&connection, false).await.unwrap();
        let mut new = f.scope();
        new.accept_rows(&f.state, &connection, &mut rows, false).await.unwrap();
        assert!(old.accept_rows(&f.state, &connection, &mut rows, true).await.is_err());
        assert!(!old.is_current(&f.state, &connection));
        assert!(!f.state.prs_seen.lock().contains_key("task"));
    }

    #[tokio::test]
    async fn round_one_direct_refresh_records_upd_6_without_sweep_notifications() {
        let f = Fixture::new().await;
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut rows = scope.all(&connection, false).await.unwrap();
        scope.accept_rows(&f.state, &connection, &mut rows, false).await.unwrap();
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(head.as_str()));
        assert!(f.state.prs_seen.lock().is_empty());
    }

    #[tokio::test]
    async fn round_one_stale_head_after_a_read_never_advances_the_point() {
        let f = Fixture::new().await;
        let initial = git::head_commit(&f.worktree).unwrap();
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut rows = scope.all(&connection, false).await.unwrap();
        git(&f.worktree, &["checkout", "-qb", "new-topic", "main"]);
        let _ = scope.accept_rows(&f.state, &connection, &mut rows, true).await;
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(initial.as_str()));
    }

    #[tokio::test]
    async fn round_one_a_displayed_or_offered_target_refuses_a_different_primary_and_pin_aba() {
        let f = Fixture::new().await;
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", false)], Some(f.item(42, "topic", &head, "dev/fork", false)));
        let connection = f.connection(true);
        let a = f.scope().checkout(&connection, "task-api", false).await.unwrap();
        let approved = a.row.action_target.as_deref().unwrap();
        assert_eq!(a.require_target(approved).unwrap().number, 7);
        f.save_pin(Some(f.pin(42)));
        let b = f.scope().checkout(&connection, "task-api", false).await.unwrap();
        assert_eq!(b.row.open().unwrap().number, 42);
        assert!(b.require_target(approved).is_err(), "the old approval would edit PR B");
        f.save_pin(None);
        let again = f.scope().checkout(&connection, "task-api", false).await.unwrap();
        assert_eq!(again.row.open().unwrap().number, 7);
        assert!(again.require_target(approved).is_err(), "the same PR number cannot hide a pin ABA");
    }

    #[tokio::test]
    async fn an_unreadable_sweep_replaces_even_settled_rows_without_saved_links() {
        use super::super::super::github::github_all_prs_from;
        let f = Fixture::new().await;
        f.save_pin(Some(f.pin(42)));
        f.state.prs_settled.lock().insert("task".into(), Instant::now());
        let rows = github_all_prs_from(
            &f.state,
            Connection::load_with(&f.state, || Err(Error::Other("secret detail not for display".into()))),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].ticket.is_none());
        let row = &rows[0].rows[0];
        assert_eq!((&row.repo[..], &row.checkout_id[..]), ("api", "task-api"));
        assert_eq!(row.error.as_deref(), Some(Error::GithubCredentialUnreadable.to_string().as_str()));
        assert_eq!(row.pin.as_ref().unwrap().url, "");
        assert!(row.pr.is_none() && row.action_target.is_none() && !row.covered_merge);
        assert!(!f.state.prs_settled.lock().contains_key("task"));
    }

    #[tokio::test]
    async fn round_one_open_prs_can_record_upd_6_before_skipping_landed_work() {
        let f = Fixture::new().await;
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut result = scope.checkout(&connection, "task-api", false).await.unwrap();
        scope.advance_rows(&f.state, &connection, std::slice::from_mut(&mut result)).await.unwrap();
        assert!(result.task_ref_landed);
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(head.as_str()));
        assert!(f.state.prs_seen.lock().is_empty());
    }

    #[tokio::test]
    async fn a_departed_topic_invalidates_the_final_create_evidence_without_moving_head() {
        use super::super::super::open_prs::require_create_evidence;
        let f = Fixture::new().await;
        commit(&f.worktree, "task-work");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(Vec::new(), None);
        let connection = f.connection(true);
        let scope = f.scope();
        let row = scope.checkout(&connection, "task-api", false).await.unwrap();
        assert!(matches!(row.publication("task").unwrap(), Publication::Create));
        assert!(require_create_evidence(&f.state, &scope, &connection, &row).is_ok());

        git(&f.worktree, &["checkout", "-qb", "late-pr-topic"]);
        commit(&f.worktree, "topic-work");
        git(&f.worktree, &["checkout", "-q", "task"]);

        assert_eq!(git::head_commit(&f.worktree).unwrap(), head);
        assert!(scope.require_current(&f.state, &connection, std::slice::from_ref(&row)).is_ok());
        assert!(require_create_evidence(&f.state, &scope, &connection, &row).is_err());
    }

    #[tokio::test]
    async fn draft_uses_the_verified_node_and_never_mutates_after_a_late_pin_change() {
        use super::super::super::pr_draft::set_draft;
        use tokio::sync::Notify;

        for change in ["pin", "origin", "neither"] {
            let f = Fixture::new().await;
            let head = git::head_commit(&f.worktree).unwrap();
            let mut pull = f.item(7, "task", &head, "acme/api", false);
            pull["node_id"] = json!("approved-node");
            let pause = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let (p, r, requests) = (pause.clone(), release.clone(), seen.clone());
            let site = Router::new()
                .route(
                    "/api/v3/repos/acme/api/pulls",
                    axum::routing::get({
                        let pull = pull.clone();
                        move || std::future::ready(Json(json!([pull])))
                    }),
                )
                .route(
                    "/api/v3/repos/acme/api/pulls/7",
                    axum::routing::get(move || async move {
                        if change != "neither" {
                            p.notify_one();
                            r.notified().await;
                        }
                        Json(pull)
                    }),
                )
                .route(
                    "/api/graphql",
                    axum::routing::post(move |Json(body): Json<Value>| async move {
                        requests.lock().unwrap().push(body);
                        Json(json!({ "data": { "convertPullRequestToDraft": { "pullRequest": { "isDraft": true } } } }))
                    }),
                );
            let listener = f.listener.lock().unwrap().take().unwrap();
            tokio::spawn(async move { axum::serve(listener, site).await.unwrap() });
            let connection = f.connection(true);
            let shown = f.scope().checkout(&connection, "task-api", false).await.unwrap();
            let target = shown.row.action_target.unwrap();
            let scope = f.scope();
            let set = set_draft(&f.state, &scope, &connection, "task-api", &target, true);
            if change != "neither" {
                let (result, ()) = tokio::join!(set, async {
                    pause.notified().await;
                    if change == "pin" {
                        f.save_pin(Some(f.pin(42)));
                    } else {
                        git(&f.worktree, &["remote", "set-url", "origin", &format!("{}/acme/right.git", f.site)]);
                    }
                    release.notify_one();
                });
                assert!(result.is_err());
                assert!(seen.lock().unwrap().is_empty());
            } else {
                assert!(set.await.unwrap());
                let mutations = seen.lock().unwrap();
                assert_eq!(mutations.len(), 1);
                assert_eq!(mutations[0]["variables"]["id"], "approved-node");
            }
        }
    }

    #[tokio::test]
    async fn drafted_screenshots_do_not_follow_a_replacement_primary() {
        use super::super::super::pr_images::show_approved_images;
        use crate::integrations::github::Image;
        use std::sync::atomic::{AtomicBool, Ordering};
        let f = Fixture::new().await;
        let head = git::head_commit(&f.worktree).unwrap();
        let pr = f.item(7, "task", &head, "acme/api", false);
        let replacement = f.item(8, "task", &head, "acme/api", false);
        let replaced = Arc::new(AtomicBool::new(false));
        let use_replacement = replaced.clone();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let seen = writes.clone();
        let router = Router::new().fallback(move |method: Method, uri: Uri| {
            let (pr, replacement, replaced, seen) =
                (pr.clone(), replacement.clone(), use_replacement.clone(), seen.clone());
            async move {
                let path = uri.path();
                if method != Method::GET {
                    seen.lock().unwrap().push((method, path.to_owned()));
                    return Json(json!({"sha": "fake-sha"})).into_response();
                }
                match path {
                    "/api/v3/repos/acme/api/pulls" => {
                        Json(json!([if replaced.load(Ordering::SeqCst) { replacement } else { pr }])).into_response()
                    }
                    "/api/v3/repos/acme/api/pulls/7" => Json(pr).into_response(),
                    "/api/v3/repos/acme/api/git/matching-refs/pr-images/7" => Json(json!([])).into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        let listener = f.listener.lock().unwrap().take().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let connection = f.connection(true);
        let scope = f.scope();
        let shown = scope.checkout(&connection, "task-api", false).await.unwrap();
        let target = shown.row.action_target.unwrap();
        let images = [Image { caption: "Fixture".into(), ext: "png", bytes: b"\x89PNG\r\n\x1a\nfixture".to_vec() }];
        let show = || show_approved_images(&f.state, &scope, &connection, "task-api", &images, &target, None);
        replaced.store(true, Ordering::SeqCst);
        assert!(show().await.is_err());
        assert!(writes.lock().unwrap().is_empty());
        replaced.store(false, Ordering::SeqCst);
        show().await.unwrap();
        let writes = writes.lock().unwrap();
        assert_eq!(writes.len(), 5);
        assert_eq!(writes.last(), Some(&(Method::PATCH, "/api/v3/repos/acme/api/pulls/7".into())));
    }

    #[tokio::test]
    async fn round_one_dirty_work_arriving_during_jira_get_prevents_the_merged_transition() {
        jira_during_lookup(false).await;
    }

    #[tokio::test]
    async fn round_two_a_departed_topic_during_the_jira_lookup_defers_the_merged_move() {
        jira_during_lookup(true).await;
    }

    #[tokio::test]
    async fn jira_follow_never_moves_the_same_key_on_another_site_or_after_reconnect() {
        use super::super::super::ticket_flow::JiraConnection;
        use crate::config::{FlowStatus, JiraConfig, TicketFlow};
        use std::sync::atomic::{AtomicUsize, Ordering};

        for changed_during_read in [false, true] {
            let f = Arc::new(Fixture::new().await);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let old_url = if changed_during_read {
                format!("{base}/browse/ACME-1")
            } else {
                "https://old-jira.example.test/browse/ACME-1".into()
            };
            let status = FlowStatus { id: "3".into(), name: "Review".into() };
            let cfg = JiraConfig {
                base_url: base,
                email: "fixture@example.invalid".into(),
                flow: [("ACME".into(), TicketFlow { review: Some(status), ..Default::default() })].into(),
                ..Default::default()
            };
            f.state
                .config
                .update(|c| {
                    c.jira = Some(cfg.clone());
                    c.tasks[0].issue_key = Some("ACME-1".into());
                    c.tasks[0].issue_url = Some(old_url.clone());
                })
                .unwrap();
            let watch = f.clone();
            let reads = Arc::new(AtomicUsize::new(0));
            let writes = Arc::new(AtomicUsize::new(0));
            let (r, w) = (reads.clone(), writes.clone());
            let router = Router::new().fallback(move |method: Method, uri: Uri| {
                let (watch, r, w) = (watch.clone(), r.clone(), w.clone());
                async move {
                    if method == Method::POST {
                        w.fetch_add(1, Ordering::SeqCst);
                        return StatusCode::NO_CONTENT.into_response();
                    }
                    r.fetch_add(1, Ordering::SeqCst);
                    if uri.path().ends_with("/transitions") {
                        if changed_during_read {
                            watch.state.config.update(|c| {
                                c.jira.as_mut().unwrap().base_url = "https://new-jira.example.test".into();
                            }).unwrap();
                        }
                        return Json(json!({"transitions":[{"id":"review","name":"Review","to":{"id":"3","name":"Review","statusCategory":{"key":"indeterminate"}}}]})).into_response();
                    }
                    Json(json!({"key":"ACME-1","fields":{"status":{"id":"1","name":"Working","statusCategory":{"key":"indeterminate"}}}})).into_response()
                }
            });
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let head = git::head_commit(&f.worktree).unwrap();
            f.serve(vec![f.item(7, "task", &head, "acme/api", false)], None);
            let connection = f.connection(true);
            let scope = f.scope();
            let evidence = scope.all(&connection, false).await.unwrap();
            let rows = [evidence[0].row.clone()];
            let jira = JiraConnection::load_with(&f.state, || Ok(TOKEN.into())).unwrap();
            let _ =
                super::super::super::ticket_flow::follow(&f.state, &jira, &scope, &connection, &rows, &evidence).await;
            let task = f.state.config.task("task").unwrap();
            assert_eq!(reads.load(Ordering::SeqCst), if changed_during_read { 2 } else { 0 });
            assert_eq!(writes.load(Ordering::SeqCst), 0);
            assert_eq!(task.issue_url.as_deref(), Some(old_url.as_str()));
            assert!(task.ticket_stage.is_none());
        }
    }

    async fn jira_during_lookup(departed_topic: bool) {
        use super::super::super::ticket_flow::JiraConnection;
        use crate::config::{FlowStatus, JiraConfig, TicketFlow};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let f = Fixture::new().await;
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let jira_config = JiraConfig {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            email: "fixture@example.invalid".into(),
            flow: [(
                "ACME".into(),
                TicketFlow { merged: Some(FlowStatus { id: "3".into(), name: "Done".into() }), ..Default::default() },
            )]
            .into(),
            ..Default::default()
        };
        f.state
            .config
            .update_local_tasks(|c| {
                c.jira = Some(jira_config.clone());
                c.tasks[0].issue_key = Some("ACME-1".into());
                c.tasks[0].issue_url = Some(format!("{}/browse/ACME-1", jira_config.base_url));
                Ok(())
            })
            .unwrap();
        let jira = JiraConnection::load_with(&f.state, || Ok(TOKEN.into())).unwrap();
        let posts = Arc::new(AtomicUsize::new(0));
        let dirty_once = Arc::new(AtomicBool::new(true));
        let path = f.worktree.join("dirty-during-jira");
        let route_path = path.clone();
        let worktree = f.worktree.clone();
        let log = posts.clone();
        let api = Router::new().fallback(move |method: Method, uri: Uri| {
            let (log, dirty_once, path, worktree) = (log.clone(), dirty_once.clone(), route_path.clone(), worktree.clone());
            async move {
                if method == Method::POST {
                    log.fetch_add(1, Ordering::SeqCst);
                    return StatusCode::NO_CONTENT.into_response();
                }
                if uri.path().ends_with("/transitions") {
                    if dirty_once.swap(false, Ordering::SeqCst) {
                        if departed_topic {
                            off_runtime(move || {
                                git(&worktree, &["checkout", "-qb", "late-topic"]);
                                commit(&worktree, "new-pr-work");
                                git(&worktree, &["checkout", "-q", "task"]);
                            }).await.unwrap();
                        } else {
                            std::fs::write(path, "incomplete work").unwrap();
                        }
                    }
                    return Json(json!({"transitions":[{"id":"close","name":"Close","to":{"id":"3","name":"Done","statusCategory":{"key":"done"}}}]})).into_response();
                }
                Json(json!({"key":"ACME-1","fields":{"status":{"id":"1","name":"Working","statusCategory":{"key":"indeterminate"}}}})).into_response()
            }
        });
        tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut evidence = scope.all(&connection, false).await.unwrap();
        let rows = scope.accept_rows(&f.state, &connection, &mut evidence, true).await.unwrap();
        assert!(super::super::super::github::task_merged(&rows));
        let follow =
            || super::super::super::ticket_flow::follow(&f.state, &jira, &scope, &connection, &rows, &evidence);
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        let moved = follow().await;
        if departed_topic {
            assert_eq!(git::head_commit(&f.worktree).unwrap(), head);
            assert_eq!(git::status(&f.worktree).unwrap().dirty_files, 0);
            let history = &scope.checkouts[0].history;
            let checkout = &scope.checkouts[0].checkout;
            let evidence = pr_branches::branch_evidence(checkout, "task", history);
            let departed_sha = git::task_branch_tip(&f.worktree, "late-topic").unwrap();
            assert!(evidence.errors.is_empty(), "{:?}", evidence.errors);
            assert!(evidence
                .candidates
                .iter()
                .any(|c| c.branch == "late-topic"
                    && c.exact_shas.as_deref() == Some(std::slice::from_ref(&departed_sha))));
            assert_eq!(posts.load(Ordering::SeqCst), 0, "a stale result posted after the newly eligible topic");
            assert!(moved.is_none());
            assert_eq!(f.state.config.task("task").unwrap().ticket_stage, None);
            return;
        }
        assert!(moved.is_none());
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        assert_eq!(f.state.config.task("task").unwrap().ticket_stage, None);
        std::fs::remove_file(path).unwrap();
        assert!(follow().await.is_some());
        assert_eq!(posts.load(Ordering::SeqCst), 1);
        assert_eq!(f.state.config.task("task").unwrap().ticket_stage.as_deref(), Some("merged"));
    }

    #[tokio::test]
    async fn round_two_departed_topic_before_application_prevents_branch_point_advancement() {
        let f = Fixture::new().await;
        let before = git::head_commit(&f.worktree).unwrap();
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut rows = scope.all(&connection, false).await.unwrap();
        git(&f.worktree, &["checkout", "-qb", "late-topic"]);
        commit(&f.worktree, "unlisted");
        git(&f.worktree, &["checkout", "-q", "task"]);
        assert_eq!(git::head_commit(&f.worktree).unwrap(), head);
        assert_eq!(git::status(&f.worktree).unwrap().dirty_files, 0);
        let rows = scope.accept_rows(&f.state, &connection, &mut rows, true).await.unwrap();
        assert!(rows[0].pr.as_ref().unwrap().merged, "keep the usable PR");
        assert!(rows[0].error.is_some(), "the original branch evidence no longer covers the checkout");
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(before.as_str()));
    }

    #[tokio::test]
    async fn round_two_unread_history_blocks_branch_advancement_even_when_candidates_are_unchanged() {
        let f = Fixture::new().await;
        let before = git::head_commit(&f.worktree).unwrap();
        commit(&f.worktree, "landed");
        let head = git::head_commit(&f.worktree).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let mut scope = f.scope();
        let mut rows = scope.all(&connection, false).await.unwrap();
        assert!(rows[0].row.error.is_none());
        assert!(rows[0].advance.is_some());
        let entry = &scope.checkouts[0];
        let branch_candidates = pr_branches::branch_evidence(&entry.checkout, "task", &entry.history).candidates;
        let log = git(&f.worktree, &["rev-parse", "--path-format=absolute", "--git-path", "logs/HEAD"]);
        std::fs::remove_file(log.trim()).unwrap();
        let actual = pr_branches::branch_evidence(&entry.checkout, "task", &entry.history);
        assert_eq!(actual.candidates, branch_candidates);
        assert!(!actual.errors.is_empty());
        let rows = scope.accept_rows(&f.state, &connection, &mut rows, true).await.unwrap();
        assert!(rows[0].error.is_some());
        assert_eq!(f.state.config.checkout("task-api").unwrap().base_commit.as_deref(), Some(before.as_str()));
    }

    #[tokio::test]
    async fn round_three_partial_finish_recovers_the_only_verified_merge_after_reload() {
        use super::super::super::tasks::finish_worktrees;

        let f = Fixture::new().await;
        commit(&f.worktree, "merged-only-in-a");
        let head = git::head_commit(&f.worktree).unwrap();
        let (right_root, other) = add_right_checkout(&f);
        let right = Path::new(&other.path);
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let first_scope = f.scope();
        let rows = first_scope.all(&connection, false).await.unwrap();
        assert!(rows[0].row.covered_merge);
        assert!(rows[1].row.pr.is_none());

        // Control: B alone, with no verified merge and no previous Finish.
        let mut control = f.state.config.read();
        control.checkouts.retain(|c| c.id == "task-right");
        let control = state(&right_root, control);
        let only_b = Scope::capture(&control, "task", Instant::now()).unwrap();
        let b_rows = only_b.all(&connection, false).await.unwrap();
        assert!(finish_worktrees(&control, &only_b, &connection, &b_rows, |_| Ok(())).is_err());
        assert!(right.is_dir());

        let partial = finish_worktrees(&f.state, &first_scope, &connection, &rows, |target| {
            if target.is_some_and(|c| c.id == "task-right") {
                return Err(Error::Other("Keep B for retry.".into()));
            }
            Ok(())
        })
        .unwrap();
        assert!(partial.iter().any(|r| r.checkout_id == other.id && !r.ok));
        assert!(!f.worktree.exists());
        assert!(right.is_dir());
        assert!(git::branch_exists(Path::new(&f.state.config.project("api").unwrap().path), "task"));
        let saved = std::fs::read(f.state.config.folder().join("config.json")).unwrap();
        let restored: AppConfig = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.checkouts.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["task-right"]);
        let restarted = state(&f.root, restored);
        let scope = Scope::capture(&restarted, "task", Instant::now()).unwrap();
        let mut fresh = scope.all(&connection, false).await.unwrap();
        assert!(fresh[0].row.pr.is_none() && fresh[0].row.branch_covered, "{:?}", fresh[0].row);
        // A receipt proves A's previous removal, not a newly reused path.
        std::fs::create_dir(&f.worktree).unwrap();
        assert!(finish_worktrees(&restarted, &scope, &connection, &fresh, |_| Ok(())).is_err());
        std::fs::remove_dir(&f.worktree).unwrap();
        // B must still match the fresh resolver snapshot at removal time.
        let late_file = right.join("late-uncommitted");
        std::fs::write(&late_file, "keep this work").unwrap();
        let late = finish_worktrees(&restarted, &scope, &connection, &fresh, |_| Ok(())).unwrap();
        assert!(late.iter().any(|r| !r.ok));
        assert!(right.is_dir());
        std::fs::remove_file(late_file).unwrap();
        fresh[0].row.error = Some("Unreadable PRs.".into());
        assert!(finish_worktrees(&restarted, &scope, &connection, &fresh, |_| Ok(())).is_err());
        fresh[0].row.error = None;
        let done = finish_worktrees(&restarted, &scope, &connection, &fresh, |_| Ok(())).unwrap();
        assert_eq!(done.len(), 1);
        assert!(done[0].ok);
        assert!(!right.exists());
        assert!(restarted.config.task("task").is_err());
        assert!(restarted.config.with(|c| c.finish_receipts.is_empty()));
        let _ = std::fs::remove_dir_all(&right_root);
    }

    #[tokio::test]
    async fn round_four_receipt_finish_refuses_a_historical_pr_opened_during_lookup() {
        use super::super::super::tasks::finish_worktrees;
        use std::sync::atomic::{AtomicBool, Ordering};

        let f = Fixture::new().await;
        commit(&f.worktree, "merged-only-in-a");
        let head = git::head_commit(&f.worktree).unwrap();
        let (right_root, other) = add_right_checkout(&f);
        let right = Path::new(&other.path);
        let release_topic = Arc::new(AtomicBool::new(false));
        let late_head = Arc::new(Mutex::new(None::<String>));
        let listener = f.listener.lock().unwrap().take().unwrap();
        let publish = release_topic.clone();
        let seen_head = late_head.clone();
        let worktree = right.to_path_buf();
        let merged = f.item(7, "task", &head, "acme/api", true);
        let mut opened = f.item(8, "late-topic", &head, "acme/right", false);
        opened["base"]["repo"]["full_name"] = json!("acme/right");
        opened["html_url"] = json!(format!("{}/acme/right/pull/8", f.site));
        let api = Router::new().fallback(move |uri: Uri| {
            let (publish, seen_head, worktree) = (publish.clone(), seen_head.clone(), worktree.clone());
            let (merged, mut opened) = (merged.clone(), opened.clone());
            async move {
                let query = uri.query().unwrap_or_default();
                match uri.path() {
                    "/api/v3/repos/acme/api/pulls" => Json(json!([merged])),
                    "/api/v3/repos/acme/right/pulls" if query.contains("head=acme%3Alate-topic&") => {
                        opened["head"]["sha"] = json!(seen_head.lock().unwrap().as_ref().unwrap());
                        Json(json!([opened]))
                    }
                    "/api/v3/repos/acme/right/pulls" => {
                        if publish.swap(false, Ordering::SeqCst) {
                            let sha = off_runtime(move || depart_topic(&worktree)).await.unwrap();
                            *seen_head.lock().unwrap() = Some(sha);
                        }
                        Json(json!([]))
                    }
                    _ => panic!("unexpected fixture request: {uri}"),
                }
            }
        });
        tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });

        let connection = f.connection(true);
        let scope = f.scope();
        let rows = scope.all(&connection, false).await.unwrap();
        let partial = finish_worktrees(&f.state, &scope, &connection, &rows, |target| {
            if target.is_some_and(|c| c.id == other.id) {
                return Err(Error::Other("Keep B for retry.".into()));
            }
            Ok(())
        })
        .unwrap();
        assert!(partial[0].ok && !partial[1].ok);
        assert!(!f.worktree.exists() && right.is_dir());

        let saved = std::fs::read(f.state.config.folder().join("config.json")).unwrap();
        let restored = state(&f.root, serde_json::from_slice(&saved).unwrap());
        let scope = Scope::capture(&restored, "task", Instant::now()).unwrap();
        assert!(restored.config.with(|c| c
            .finish_receipts
            .get("task")
            .is_some_and(|receipt| { receipt.matches(c, &scope.task) && receipt.worktree_still_absent() })));
        let initial = Local::read(&other, "task", &f.api).unwrap();
        assert_eq!(initial.dirty, 0);
        release_topic.store(true, Ordering::SeqCst);
        let rows = scope.all(&connection, false).await.unwrap();
        assert_eq!(Local::read(&other, "task", &f.api).unwrap(), initial);
        assert!(validate_finish(&rows, true).is_ok(), "the receipt would allow the stale rows");
        assert!(validate_finish(&rows, false).is_err(), "the receipt is the only verified merge");
        let history = pr_branches::branch_evidence(&other, "task", &scope.checkouts[0].history);
        assert!(history.errors.is_empty());
        let sha = late_head.lock().unwrap().clone().unwrap();
        assert!(history
            .candidates
            .iter()
            .any(|c| c.branch == "late-topic" && c.exact_shas.as_deref() == Some(std::slice::from_ref(&sha))));
        assert!(rows[0].branches.as_ref() != Some(&history.stamp()));

        let checkpoints = std::cell::Cell::new(0);
        let result = finish_worktrees(&restored, &scope, &connection, &rows, |_| {
            checkpoints.set(checkpoints.get() + 1);
            Ok(())
        });
        assert!(result.is_err_and(|e| e.to_string() == stale().to_string()), "Finish accepted stale branch evidence.");
        assert_eq!(checkpoints.get(), 1, "Finish should refuse before reaching the post-shutdown guard.");
        assert!(right.is_dir() && restored.config.checkout(&other.id).is_ok());
        assert!(restored.config.with(|c| c.finish_receipts.contains_key("task")));

        let fresh = scope.all(&connection, false).await.unwrap();
        assert_eq!(fresh[0].row.open().unwrap().number, 8);
        assert!(finish_worktrees(&restored, &scope, &connection, &fresh, |_| Ok(())).is_err());
        let _ = std::fs::remove_dir_all(&right_root);
    }

    #[tokio::test]
    async fn round_four_normal_finish_keeps_the_worktree_with_new_history_at_removal() {
        finish_two_worktrees(true).await;
    }

    #[tokio::test]
    async fn round_four_normal_finish_does_not_read_an_already_removed_worktree() {
        finish_two_worktrees(false).await;
    }

    async fn finish_two_worktrees(change_history: bool) {
        use super::super::super::tasks::finish_worktrees;

        let f = Fixture::new().await;
        commit(&f.worktree, "merged-only-in-a");
        let head = git::head_commit(&f.worktree).unwrap();
        let (right_root, other) = add_right_checkout(&f);
        let right = Path::new(&other.path);
        let initial = Local::read(&other, "task", &f.api).unwrap();
        f.serve(vec![f.item(7, "task", &head, "acme/api", true)], None);
        let connection = f.connection(true);
        let scope = f.scope();
        let rows = scope.all(&connection, false).await.unwrap();
        assert!(validate_finish(&rows, false).is_ok());
        let results = finish_worktrees(&f.state, &scope, &connection, &rows, |target| {
            if target.is_some_and(|c| c.id == other.id) {
                assert!(!f.worktree.exists(), "A was not yet removed.");
                if change_history {
                    depart_topic(right);
                    assert_eq!(Local::read(&other, "task", &f.api).unwrap(), initial);
                }
            }
            Ok(())
        })
        .unwrap();
        if !change_history {
            assert_eq!(results.len(), 2);
            assert!(results.iter().all(|r| r.ok), "{results:?}");
            assert!(!right.exists() && f.state.config.task("task").is_err());
            let _ = std::fs::remove_dir_all(&right_root);
            return;
        }
        assert!(results[0].ok && !results[1].ok, "{results:?}");
        assert_eq!(results[1].detail, format!("Worktree kept: {}", stale()));
        assert!(right.is_dir() && f.state.config.checkout(&other.id).is_ok());
        assert!(f.state.config.checkout("task-api").is_err());
        assert!(f.state.config.with(|c| c.finish_receipts.contains_key("task")));

        // Even though A has been removed, the retry has no missing worktree
        // to read: the receipt and B's fresh evidence are enough.
        let scope = f.scope();
        let fresh = scope.all(&connection, false).await.unwrap();
        let done = finish_worktrees(&f.state, &scope, &connection, &fresh, |_| Ok(())).unwrap();
        assert_eq!(done.len(), 1);
        assert!(done[0].ok && !right.exists());
        let _ = std::fs::remove_dir_all(&right_root);
    }

    fn add_right_checkout(f: &Fixture) -> (PathBuf, Checkout) {
        let (root, config) = setup();
        let mut project = config.projects[0].clone();
        project.id = "right".into();
        project.name = "right".into();
        let repo = Path::new(&project.path);
        git(repo, &["remote", "set-url", "origin", &format!("{}/acme/right.git", f.site)]);
        let mut checkout = f.state.config.checkout("task-api").unwrap();
        checkout.id = "task-right".into();
        checkout.project_id = project.id.clone();
        checkout.path = f.worktree.with_file_name("right").to_string_lossy().into_owned();
        git(repo, &["worktree", "add", "-qb", "task", &checkout.path]);
        checkout.base_commit = Some(git::head_commit(Path::new(&checkout.path)).unwrap());
        f.state
            .config
            .update_local_tasks(|c| {
                c.projects.push(project);
                c.checkouts.push(checkout.clone());
                Ok(())
            })
            .unwrap();
        (root, checkout)
    }

    fn depart_topic(worktree: &Path) -> String {
        git(worktree, &["checkout", "-qb", "late-topic"]);
        commit(worktree, "new-pr-work");
        let sha = git::head_commit(worktree).unwrap();
        git(worktree, &["checkout", "-q", "task"]);
        sha
    }
}
