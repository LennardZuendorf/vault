//! Tabs nobody uses are closed until they are needed again (BRW-6).
//!
//! Every task's tabs stayed open in Chrome for as long as the app ran: with
//! twenty tasks that was 1.2–1.5 GB and 5–12% of a core all the time, for
//! pages nobody was looking at. A task's tabs left unused for `IDLE` are
//! closed, their addresses kept in the config as on quitting, and
//! `Browser::page` opens them again the next time an agent or the panel
//! needs them. With no tabs left, Chrome itself stops.

use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Manager, Runtime};

use super::{changed, tabs, Browser, Inner};
use crate::commands::AppState;

/// How long a task's tabs are kept open with nobody using them.
const IDLE: Duration = Duration::from_secs(15 * 60);

/// How often the tabs are looked at. A minute late is nothing next to
/// fifteen.
const ROUND: Duration = Duration::from_secs(60);

/// What `shutdown` gives Chrome to close before it is killed (BRW-7).
const GRACE: Duration = Duration::from_secs(3);

/// The rounds, on a thread of their own: stopping Chrome waits for it, and
/// waiting belongs on neither the main thread nor the async runtime.
pub fn spawn<R: Runtime>(app: AppHandle<R>) -> std::io::Result<()> {
    std::thread::Builder::new().name("browser-sleep".into()).spawn(move || loop {
        std::thread::sleep(ROUND);
        app.state::<AppState>().browser.sleep_idle(&app, IDLE);
    })?;
    Ok(())
}

impl Inner {
    /// The tasks whose tabs nobody has used for `idle` and nobody is
    /// waiting on.
    fn idle_tasks(&self, now: Instant, idle: Duration) -> Vec<String> {
        // A window a page opened, still on its way to becoming a tab: it
        // would land in a task closed under it.
        if self.adopting > 0 {
            return Vec::new();
        }
        self.tabs
            .iter()
            .filter(|(task, tabs)| {
                let task = task.as_str();
                let shown = self.watch.as_ref().is_some_and(|w| w.task == task)
                    || self.wanted.as_ref().is_some_and(|(_, t)| t == task);
                let driven = self.drivers.get(task).is_some_and(|d| d.calls > 0);
                // A dialog stops the page's scripts until someone answers it;
                // closed, the answer would go nowhere (BRW-13).
                let asking = tabs.list.iter().any(|t| t.dialog.is_some());
                // The user took the tab over, to sign in perhaps (BRW-12).
                let held = self.held.contains(task);
                let recent = self.used.get(task).is_none_or(|at| now.saturating_duration_since(*at) < idle);
                !(shown || driven || asking || held || recent)
            })
            .map(|(task, _)| task.clone())
            .collect()
    }
}

impl Browser {
    /// Close the tabs of every task nobody has used for `idle`, keeping
    /// where they were; stop Chrome once none are left.
    pub fn sleep_idle<R: Runtime>(&self, app: &AppHandle<R>, idle: Duration) {
        // Held throughout, so no tab is made, and no Chrome started on the
        // same profile, while one is being put away. Busy: next round.
        let Ok(_making) = self.making.try_lock() else { return };
        let now = Instant::now();
        let (slept, cdp, stopped) = {
            let mut inner = self.inner.lock();
            let Some(cdp) = inner.running.as_ref().map(|r| r.cdp.clone()) else { return };
            // Tabs never used through `page` (none should be): from now.
            let unseen: Vec<String> = inner.tabs.keys().filter(|t| !inner.used.contains_key(*t)).cloned().collect();
            for task in unseen {
                inner.used.insert(task, now);
            }
            let tasks = inner.idle_tasks(now, idle);
            // Out of the tabs before their targets close, so the reader
            // takes Chrome's "destroyed" for no tab of the app's, and the
            // addresses kept stay as they are (`events.rs`).
            let slept: Vec<_> =
                tasks.into_iter().filter_map(|task| inner.tabs.remove(&task).map(|tabs| (task, tabs))).collect();
            let unused =
                inner.tabs.is_empty() && inner.watch.is_none() && inner.wanted.is_none() && inner.adopting == 0;
            let stopped = if unused {
                inner.heard.clear();
                inner.running.take()
            } else {
                None
            };
            (slept, cdp, stopped)
        };
        for (task, list) in &slept {
            tabs::keep(app, task, list.saved());
        }
        match stopped {
            // As `shutdown` does. The reader then hears the pipe close and
            // finds no browser running, so it forgets nothing.
            Some(r) => {
                r.cdp.send("Browser.close", json!({}), None);
                r.process.lock().stop(GRACE);
            }
            None => {
                for t in slept.iter().flat_map(|(_, list)| &list.list) {
                    cdp.send("Target.closeTarget", json!({ "targetId": t.target }), None);
                }
            }
        }
        for (task, _) in &slept {
            changed(app, task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::control::Driver;
    use crate::browser::tabs::Tabs;
    use crate::browser::tests::{app_with_task, serve, tab, HOME};
    use crate::browser::Dialog;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn only_tabs_nobody_uses_or_waits_on_are_put_to_sleep() {
        let then = Instant::now();
        let mut inner = Inner::default();
        for task in ["idle", "recent", "shown", "driven", "held", "asking"] {
            inner.tabs.insert(task.into(), Tabs::new(vec![tab(task)], 0));
            inner.used.insert(task.into(), then);
        }
        let now = then + 16 * MINUTE;
        inner.used.insert("recent".into(), now - MINUTE);
        inner.wanted = Some((1, "shown".into()));
        inner
            .drivers
            .insert("driven".into(), Driver { pane: "p".into(), agent: "Claude Code".into(), calls: 1, last_at: 0 });
        inner.held.insert("held".into());
        if let Some(t) = inner.tabs.get_mut("asking").and_then(Tabs::active_mut) {
            t.dialog = Some(Dialog { kind: "alert".into(), message: "Hi".into(), default_prompt: String::new() });
        }

        assert_eq!(inner.idle_tasks(now, 15 * MINUTE), vec!["idle".to_string()]);
        assert!(inner.idle_tasks(then + 10 * MINUTE, 15 * MINUTE).is_empty(), "not yet");

        inner.adopting = 1;
        assert!(inner.idle_tasks(now, 15 * MINUTE).is_empty(), "a window is on its way");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sleeping_task_s_tab_comes_back_at_its_page() {
        let Some((app, _root, _pane)) = app_with_task() else { return };
        let state = app.state::<AppState>();
        let base = serve(HOME).await;
        let page = state.browser.page(app.handle(), "t1").await.unwrap();
        page.navigate(&format!("{base}/two")).await.unwrap();
        // Its address as the tab heard it, which is what is kept.
        for _ in 0..50 {
            if state.browser.state_of("t1").is_some_and(|s| s.0.ends_with("/two")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Off the runtime, as on its own thread: stopping Chrome waits.
        let handle = app.handle().clone();
        tokio::task::spawn_blocking(move || handle.state::<AppState>().browser.sleep_idle(&handle, Duration::ZERO))
            .await
            .unwrap();

        assert!(state.browser.state_of("t1").is_none(), "its tab closed");
        assert!(state.browser.inner.lock().running.is_none(), "and Chrome with it, having no other");
        let kept = tabs::saved_tabs(&state.config.read(), "t1");
        assert_eq!(kept.urls, vec![format!("{base}/two")]);

        state.browser.page(app.handle(), "t1").await.unwrap();
        assert_eq!(state.browser.state_of("t1").map(|s| s.0), Some(format!("{base}/two")));
        state.browser.shutdown(GRACE);
    }
}
