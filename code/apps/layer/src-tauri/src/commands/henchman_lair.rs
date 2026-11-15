//! What the lair keeps (HEN-14): the needs the Henchman raised that the
//! user has not closed, and a line for each thing it did for them. Kept in
//! `henchman.json` beside the config, so closing Layer loses neither: the
//! user comes back to the same column they left.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::error::Result;
use crate::mcp::arg;

use super::henchman::is_caller;
use super::henchman_need::Need;
use super::henchman_tools::{cut, pane_name};
use super::panes::CHAT_TASK_ID;
use super::AppState;

/// Lines of "Done for you" kept: the column shows five, the rest is history.
const DID_CAP: usize = 100;
/// Needs kept open at most. Past it the oldest goes: it was raised, bannered
/// and left, and twenty unanswered say the Henchman asks too much.
const NEEDS_CAP: usize = 20;
/// How much of a message a line quotes.
const QUOTES: usize = 80;

/// One thing the Henchman did for the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Did {
    pub at: DateTime<Utc>,
    /// The task it was for, which a click opens.
    #[serde(default)]
    pub task_id: Option<String>,
    /// Its ticket key, or its name.
    #[serde(default)]
    pub key: Option<String>,
    pub what: String,
}

/// The lair as the window shows it, and as `henchman.json` holds it.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lair {
    #[serde(default)]
    pub needs: Vec<Need>,
    /// Newest first.
    #[serde(default)]
    pub did: Vec<Did>,
}

impl Lair {
    fn log(&mut self, did: Did) {
        self.did.insert(0, did);
        self.did.truncate(DID_CAP);
    }

    pub(super) fn add_need(&mut self, need: Need) {
        self.needs.push(need);
        let over = self.needs.len().saturating_sub(NEEDS_CAP);
        self.needs.drain(..over);
    }

    fn close(&mut self, id: &str) -> bool {
        let before = self.needs.len();
        self.needs.retain(|n| n.id != id);
        self.needs.len() != before
    }

    /// A need about a task that is gone (finished, removed) has nobody to
    /// act on it any more.
    pub(super) fn forget_gone(&mut self, tasks: &HashSet<String>) -> bool {
        let before = self.needs.len();
        self.needs.retain(|n| n.task_id.as_ref().is_none_or(|t| tasks.contains(t)));
        self.needs.len() != before
    }
}

struct Kept {
    path: PathBuf,
    lair: Lair,
}

impl Kept {
    /// A missing file is an empty lair. One that cannot be read is kept
    /// beside it, since the next write replaces it.
    fn load(dir: &Path) -> Self {
        let path = dir.join("henchman.json");
        let lair = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                let kept = path.with_extension("json.unreadable");
                let _ = std::fs::copy(&path, &kept);
                eprintln!("henchman.json could not be read ({e}); the old one is at {}", kept.display());
                Lair::default()
            }),
            Err(_) => Lair::default(),
        };
        Kept { path, lair }
    }

    /// Write it off the caller's thread: changes come from the MCP server's
    /// runtime and the attention pass, neither of which may wait on the
    /// disk. Each write knows its number, and one overtaken by a newer
    /// write skips, so an older lair never lands last.
    fn save(&self) {
        let seq = WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        let path = self.path.clone();
        let bytes = serde_json::to_vec_pretty(&self.lair);
        let write = move || {
            let _disk = DISK.lock();
            if WRITES.load(Ordering::SeqCst) != seq {
                return;
            }
            if let Err(e) = bytes.map_err(Into::into).and_then(|b| crate::config::write_whole(&path, &b)) {
                eprintln!("henchman.json could not be written: {e}");
            }
        };
        if let Err(e) = std::thread::Builder::new().name("henchman-lair".into()).spawn(write) {
            eprintln!("henchman.json could not be written: {e}");
        }
    }
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
static WRITES: AtomicU64 = AtomicU64::new(0);
static DISK: Mutex<()> = Mutex::new(());

/// Change the lair; `f` says whether it did. A change is written and sent
/// to the window as `henchman:lair`.
pub(super) fn change<R: Runtime, T>(app: &AppHandle<R>, state: &AppState, f: impl FnOnce(&mut Lair) -> (T, bool)) -> T {
    let (out, changed) = {
        let mut kept = KEPT.lock();
        let kept = kept.get_or_insert_with(|| Kept::load(state.config.folder()));
        let (out, changed) = f(&mut kept.lair);
        if changed {
            kept.save();
        }
        (out, changed.then(|| kept.lair.clone()))
    };
    if let Some(lair) = changed {
        let _ = app.emit("henchman:lair", lair);
    }
    out
}

/// The lair, for the window when it opens.
#[tauri::command]
pub async fn henchman_lair(app: AppHandle) -> Result<Lair> {
    let handle = app.clone();
    super::blocking(app, move |state| Ok(change(&handle, state, |lair| (lair.clone(), false)))).await
}

/// The user closes a need: it is done, or not theirs after all.
#[tauri::command]
pub async fn henchman_need_done(app: AppHandle, id: String) -> Result<Lair> {
    let handle = app.clone();
    super::blocking(app, move |state| {
        Ok(change(&handle, state, |lair| {
            let closed = lair.close(&id);
            (lair.clone(), closed)
        }))
    })
    .await
}

/// A task by its id or ticket key, as its id and how the user names it.
pub(super) fn task_named(state: &AppState, task: &str) -> (Option<String>, String) {
    state
        .config
        .with(|c| {
            c.tasks
                .iter()
                .find(|t| t.id == task || t.issue_key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(task)))
                .map(|t| (Some(t.id.clone()), t.issue_key.clone().unwrap_or_else(|| t.name.clone())))
        })
        .unwrap_or_else(|| (None, task.to_string()))
}

/// Whether a tool call is one "Done for you" lists when the Henchman makes
/// it: what changes something, never what only reads.
pub fn henchman_logs(name: &str, args: &Value) -> bool {
    matches!(
        name,
        "pane_tell" | "pane_restart" | "task_work" | "chat_start" | "order_add" | "order_remove" | "leave_alone"
    ) || name == "start_work"
        || args.get("confirm").and_then(Value::as_bool) == Some(true)
}

/// After a tool call succeeded: if the Henchman made it, a line in "Done
/// for you" (HEN-14).
pub fn henchman_did(app: &AppHandle, caller: Option<&str>, name: &str, args: &Value, result: &Value) {
    let state = app.state::<AppState>();
    if !is_caller(&state, caller) {
        return;
    }
    // The pane it acted on: a restart's new one, or the one it named.
    let pane = result
        .get("pane_id")
        .and_then(Value::as_str)
        .or_else(|| arg(args, "pane_id"))
        .and_then(|id| state.ptys.info(id).ok());
    let chat = pane.as_ref().filter(|p| p.task_id == CHAT_TASK_ID).map(pane_name);
    let (task_id, key) = match pane.as_ref().filter(|p| p.task_id != CHAT_TASK_ID) {
        Some(p) => task_named(&state, &p.task_id),
        None => match arg(args, "task").or_else(|| arg(args, "issue_key")).or_else(|| arg(args, "key")) {
            Some(task) => task_named(&state, task),
            None => (None, String::new()),
        },
    };
    let Some(what) = what(name, args, result, chat.as_deref()) else { return };
    let did = Did { at: Utc::now(), task_id, key: Some(key).filter(|k| !k.is_empty()), what };
    change(app, &state, |lair| {
        lair.log(did);
        ((), true)
    });
}

/// The line for one call, in the user's words.
fn what(name: &str, args: &Value, result: &Value, chat: Option<&str>) -> Option<String> {
    let text = || arg(args, "text").map(|t| cut(t.trim(), QUOTES)).unwrap_or_default();
    Some(match name {
        "pane_tell" => match chat {
            Some(chat) => format!("told the chat “{chat}”: {}", text()),
            None => format!("told its agent: {}", text()),
        },
        "pane_restart" => match chat {
            Some(chat) => format!("restarted the chat “{chat}”"),
            None => "restarted its agent, which had exited".into(),
        },
        "task_work" => {
            let asked = args.get("asked").and_then(Value::as_bool) == Some(true);
            let how = if result.get("started").is_some() {
                "started its agent"
            } else if result.get("picked_up").is_some() {
                "picked its agent up"
            } else {
                "put its agent on it"
            };
            let asked = if asked { ", as you asked" } else { "" };
            format!("{how}{asked}: {}", text())
        }
        "chat_start" => format!("started a chat: {}", text()),
        "order_add" => format!("kept your order: {}", text()),
        "order_remove" => "dropped a standing order".into(),
        "leave_alone" => match args.get("alone").and_then(Value::as_bool).unwrap_or(true) {
            true => "leaves it alone, as you said".into(),
            false => "drives it again".into(),
        },
        "start_work" => "started work on it, as you asked".into(),
        other if args.get("confirm").and_then(Value::as_bool) == Some(true) => {
            format!("{}, as you asked", other.replace('_', " "))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn need(id: &str, task: Option<&str>) -> Need {
        Need {
            id: id.into(),
            at: Utc::now(),
            pane_id: "h".into(),
            task_id: task.map(str::to_string),
            key: Some("ACME-12".into()),
            text: "needs the staging password".into(),
            seen: false,
        }
    }

    #[test]
    fn the_lair_reads_back_what_it_wrote_and_an_old_file_without_lines_still_reads() {
        let mut lair = Lair::default();
        lair.add_need(need("n1", Some("t1")));
        lair.log(Did { at: Utc::now(), task_id: Some("t1".into()), key: Some("ACME-12".into()), what: "x".into() });
        let raw = serde_json::to_string(&lair).expect("written");
        assert_eq!(serde_json::from_str::<Lair>(&raw).ok(), Some(lair));
        assert_eq!(serde_json::from_str::<Lair>(r#"{"needs":[]}"#).ok(), Some(Lair::default()));
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_and_the_lair_starts_empty() {
        let dir = std::env::temp_dir().join(format!("vl-henchman-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("made");
        std::fs::write(dir.join("henchman.json"), "not json").expect("written");
        assert_eq!(Kept::load(&dir).lair, Lair::default());
        assert_eq!(std::fs::read_to_string(dir.join("henchman.json.unreadable")).ok().as_deref(), Some("not json"));
    }

    #[test]
    fn the_newest_line_comes_first_and_old_ones_and_old_needs_fall_off() {
        let mut lair = Lair::default();
        for n in 0..DID_CAP + 5 {
            lair.log(Did { at: Utc::now(), task_id: None, key: None, what: format!("did {n}") });
        }
        assert_eq!(lair.did.len(), DID_CAP);
        assert_eq!(lair.did[0].what, format!("did {}", DID_CAP + 4));
        for n in 0..NEEDS_CAP + 2 {
            lair.add_need(need(&format!("n{n}"), None));
        }
        assert_eq!(lair.needs.len(), NEEDS_CAP);
        assert_eq!(lair.needs[0].id, "n2", "the oldest go");
    }

    #[test]
    fn a_need_closes_when_the_user_says_done_or_its_task_is_gone() {
        let mut lair = Lair::default();
        lair.add_need(need("n1", Some("t1")));
        lair.add_need(need("n2", Some("t2")));
        lair.add_need(need("n3", None));
        assert!(lair.close("n1"));
        assert!(!lair.close("n1"), "closed already");
        assert!(lair.forget_gone(&HashSet::from(["t1".to_string()])));
        assert_eq!(lair.needs.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["n3"]);
    }

    #[test]
    fn each_thing_the_henchman_does_reads_as_a_line_and_reading_is_not_one() {
        let tell = json!({ "pane_id": "p1", "text": "Go on with step 3." });
        assert_eq!(what("pane_tell", &tell, &json!({}), None).as_deref(), Some("told its agent: Go on with step 3."));
        assert_eq!(
            what("pane_tell", &tell, &json!({}), Some("Tea")).as_deref(),
            Some("told the chat “Tea”: Go on with step 3.")
        );
        let work = json!({ "task": "ACME-12", "text": "Fix the failing lint.", "asked": true });
        assert_eq!(
            what("task_work", &work, &json!({ "started": "Claude Code" }), None).as_deref(),
            Some("started its agent, as you asked: Fix the failing lint.")
        );
        let merge = json!({ "task": "ACME-12", "confirm": true });
        assert_eq!(what("pr_merge", &merge, &json!({}), None).as_deref(), Some("pr merge, as you asked"));
        assert_eq!(what("pane_read", &json!({}), &json!({}), None), None);
        assert!(!henchman_logs("work_overview", &json!({})));
        assert!(henchman_logs("task_work", &json!({})));
    }
}
