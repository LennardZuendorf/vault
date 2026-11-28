#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::commands::{panes, panes_restore, projects};
    use crate::config::item_details::ItemDetails;
    use crate::config::{SavedPane, SlackThread};
    use serde_json::json;

    struct Fixture {
        root: PathBuf,
        state: AppState,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("layer-details-lifecycle-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let state = state(&root, vec![]);
            state
                .config
                .update(|c| {
                    c.tag_catalog = vec!["Review".into()];
                    for id in ["ordinary", "slack", "other"] {
                        c.saved_panes.push(
                            serde_json::from_value::<SavedPane>(json!({
                                "id":id, "task_id":"chat", "kind":"agent", "agent_id":"claude",
                                "cwd":format!("/old/_chat/{id}"), "acp":true, "session":format!("session-{id}")
                            }))
                            .unwrap(),
                        );
                        c.chat_details.insert(format!("/old/_chat/{id}"), details());
                    }
                    c.slack_threads.push(
                        serde_json::from_value::<SlackThread>(json!({
                            "channel":"C1", "ts":"1.2", "room":"/old/_chat/slack", "link":"https://slack/thread"
                        }))
                        .unwrap(),
                    );
                })
                .unwrap();
            std::fs::write(root.join("history.jsonl"), "saved transcript\n").unwrap();
            Self { root, state }
        }

        fn disk(&self) -> AppConfig {
            serde_json::from_slice(&std::fs::read(self.root.join("config.json")).unwrap()).unwrap()
        }

        fn assert_kept(&self) {
            let c = self.disk();
            assert_eq!(c.tag_catalog, ["Review"]);
            assert_eq!(c.chat_details["/old/_chat/other"], details());
            assert!(c.saved_panes.iter().any(|p| p.id == "other" && p.session.as_deref() == Some("session-other")));
            assert_eq!(c.slack_threads[0].link, "https://slack/thread");
            assert_eq!(std::fs::read_to_string(self.root.join("history.jsonl")).unwrap(), "saved transcript\n");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn details() -> ItemDetails {
        ItemDetails { display_name: Some("Alias".into()), tags: vec!["Review".into()], color: None }
    }

    #[test]
    fn ordinary_close_cleans_details_even_after_the_live_pane_has_gone() {
        let f = Fixture::new();
        panes::close_pane_inner(&f.state, "ordinary").unwrap();
        panes::close_pane_inner(&f.state, "ordinary").unwrap();
        let c = f.disk();
        assert!(!c.chat_details.contains_key("/old/_chat/ordinary"));
        assert!(!c.saved_panes.iter().any(|p| p.id == "ordinary"));
        f.assert_kept();
    }

    #[test]
    fn slack_linked_close_keeps_room_details_for_reopening() {
        let f = Fixture::new();
        panes::close_pane_inner(&f.state, "slack").unwrap();
        assert_eq!(f.disk().chat_details["/old/_chat/slack"], details());
        assert!(!f.disk().saved_panes.iter().any(|p| p.id == "slack"));
        f.assert_kept();
    }

    #[test]
    fn explicit_waiting_forget_cleans_ordinary_and_slack_details_only() {
        let f = Fixture::new();
        f.state
            .config
            .update(|c| {
                for p in c.saved_panes.iter_mut().filter(|p| p.id != "other") {
                    p.waiting = true;
                }
            })
            .unwrap();
        for id in ["ordinary", "slack", "other", "missing"] {
            panes_restore::forget_waiting_pane_inner(&f.state, id).unwrap();
        }
        let c = f.disk();
        assert!(!c.chat_details.contains_key("/old/_chat/ordinary"));
        assert!(!c.chat_details.contains_key("/old/_chat/slack"));
        assert_eq!(c.saved_panes.len(), 1);
        f.assert_kept();
    }

    #[test]
    fn removing_task_records_cleans_details_for_delete_and_finish() {
        for finished in [false, true] {
            let f = Fixture::new();
            f.state
                .config
                .update(|c| {
                    for id in ["gone", "kept"] {
                        c.tasks.push(
                            serde_json::from_value(json!({
                                "id":id, "name":"Original", "branch":"original", "root":f.root.join(id),
                                "created_at":"2026-01-01T00:00:00Z"
                            }))
                            .unwrap(),
                        );
                        c.task_details.insert(id.into(), details());
                    }
                })
                .unwrap();
            assert!(delete_task_inner(&f.state, "gone".into(), false, finished).unwrap().is_empty());
            let c = f.disk();
            assert!(!c.task_details.contains_key("gone"));
            assert_eq!(c.task_details["kept"], details());
            assert_eq!(c.tasks.len(), 1);
            f.assert_kept();
        }
    }

    #[test]
    fn dropping_the_last_repository_cleans_only_the_dropped_tasks_details() {
        let f = Fixture::new();
        let api = github_and_clone(&f.root, "api", &["main"]);
        let web = github_and_clone(&f.root, "web", &["main"]);
        f.state
            .config
            .update(|c| {
                c.projects = vec![project("api", &api, "main"), project("web", &web, "main")];
            })
            .unwrap();
        let gone = task_on(&f.state, "api", "main");
        let mut kept = gone.clone();
        kept.id = "kept".into();
        let mut empty = kept.clone();
        empty.id = "already-empty".into();
        f.state
            .config
            .update(|c| {
                c.tasks.extend([kept.clone(), empty.clone()]);
                let mut checkout = c.checkouts[0].clone();
                checkout.id = "second".into();
                checkout.task_id = kept.id.clone();
                checkout.project_id = "web".into();
                c.checkouts.push(checkout);
                for id in [&gone.id, &kept.id, &empty.id] {
                    c.task_details.insert(id.clone(), details());
                }
            })
            .unwrap();
        projects::remove_project_inner(&f.state, "api").unwrap();
        let c = f.disk();
        assert!(!c.task_details.contains_key(&gone.id));
        assert_eq!(c.task_details[&kept.id], details());
        assert_eq!(c.task_details[&empty.id], details());
        assert_eq!(c.tasks.len(), 2);
        f.assert_kept();
    }

    #[test]
    fn a_refused_delete_keeps_task_details_and_its_record() {
        let f = Fixture::new();
        let api = github_and_clone(&f.root, "api", &["main"]);
        f.state.config.update(|c| c.projects.push(project("api", &api, "main"))).unwrap();
        let task = task_on(&f.state, "api", "main");
        f.state
            .config
            .update(|c| {
                c.task_details.insert(task.id.clone(), details());
            })
            .unwrap();
        let checkout = &f.state.config.checkouts_of(&task.id)[0];
        std::fs::write(Path::new(&checkout.path).join("uncommitted.txt"), "keep me").unwrap();
        let results = delete_task_inner(&f.state, task.id.clone(), false, false).unwrap();
        assert!(results.iter().any(|r| !r.ok));
        assert_eq!(f.disk().task_details[&task.id], details());
        assert!(f.disk().tasks.iter().any(|t| t.id == task.id));
        f.assert_kept();
    }

    #[test]
    fn removing_a_checkout_keeps_details_while_the_task_record_exists() {
        let f = Fixture::new();
        let api = github_and_clone(&f.root, "api", &["main"]);
        f.state.config.update(|c| c.projects.push(project("api", &api, "main"))).unwrap();
        let task = task_on(&f.state, "api", "main");
        f.state
            .config
            .update(|c| {
                c.task_details.insert(task.id.clone(), details());
            })
            .unwrap();
        let checkout = &f.state.config.checkouts_of(&task.id)[0];
        remove_checkout_inner(&f.state, checkout.id.clone(), false).unwrap();
        assert_eq!(f.disk().task_details[&task.id], details());
        assert!(f.disk().tasks.iter().any(|t| t.id == task.id));
        f.assert_kept();
    }
}
