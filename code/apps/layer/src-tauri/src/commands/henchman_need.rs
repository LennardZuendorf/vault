//! The one way the Henchman reaches the user (HEN-13). While one runs, the
//! agents it hears of finish and exit without a banner or a count on the
//! dock: it handles them. When something really needs the user it calls
//! `need_you`: a banner, a line in the message center, and a count on the
//! dock until they open its conversation. Read by the attention pass
//! (`attention.rs`), which owns the banners and the dock.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::messages::{Kind, Level, New};
use crate::pty::{Activity, PaneInfo, PaneKind};
use crate::target::Target;

use super::henchman::is_henchman;
use super::henchman_feed::Heard;
use super::henchman_tools::cut;
use super::panes::CHAT_TASK_ID;
use super::AppState;

/// The same need is not raised again within this: the user was told.
const AGAIN: Duration = Duration::from_secs(12 * 60 * 60);
/// Needs raised within `HOUR` at most: past it, the Henchman is pinging
/// about what isn't the user's, and the rest waits in its conversation.
const PER_HOUR: usize = 6;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// How much of a need a banner says.
const SAYS: usize = 240;

/// One thing the Henchman needs the user for.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    /// The Henchman's pane, which a click opens.
    pane_id: String,
    /// The task's ticket key, or its name.
    key: Option<String>,
    text: String,
}

impl Need {
    fn body(&self) -> String {
        match &self.key {
            Some(key) => format!("{key}: {}", self.text),
            None => self.text.clone(),
        }
    }

    fn target(&self) -> String {
        Target::Pane { task: CHAT_TASK_ID, pane: &self.pane_id }.to_string()
    }

    /// For the message center, which keeps it whatever the switch (MSG-2).
    pub fn record(&self) -> New {
        New {
            kind: Kind::Agent,
            level: Level::Info,
            title: "Henchman".into(),
            body: self.body(),
            target: Some(self.target()),
        }
    }

    /// A banner's title, body and what a click opens: the Henchman's tab.
    pub fn banner(&self) -> (String, String, String) {
        ("The Henchman needs you".into(), self.body(), self.target())
    }
}

#[derive(Default)]
struct Needs {
    /// Raised and not yet bannered.
    new: Vec<Need>,
    /// When each need still open was raised: the count on the dock.
    open: Vec<DateTime<Utc>>,
    /// What was raised lately, by its words, against saying it twice.
    said: VecDeque<(String, Instant)>,
}

impl Needs {
    const fn new() -> Self {
        Needs { new: Vec::new(), open: Vec::new(), said: VecDeque::new() }
    }

    fn raise(&mut self, need: Need, now: Instant, at: DateTime<Utc>) -> Result<()> {
        self.said.retain(|(_, when)| now.duration_since(*when) < AGAIN);
        let words = words(&need.body());
        if self.said.iter().any(|(w, _)| *w == words) {
            return Err(Error::Other(
                "you told the user this already: don't raise it again, it waits in your conversation".into(),
            ));
        }
        if self.said.iter().filter(|(_, when)| now.duration_since(*when) < HOUR).count() >= PER_HOUR {
            return Err(Error::Other(format!(
                "you reached the user {PER_HOUR} times this hour: keep the rest for when they answer"
            )));
        }
        self.said.push_back((words, now));
        self.open.push(at);
        self.new.push(need);
        Ok(())
    }

    /// The needs to banner now, and how many are open. They close once the
    /// user has looked at the Henchman's conversation after the turn that
    /// raised them; with no Henchman, nobody holds them open.
    fn take(&mut self, henchman: Option<&PaneInfo>) -> (usize, Vec<Need>) {
        match henchman {
            Some(h) if h.activity == Activity::Idle => self.open.retain(|at| *at > h.activity_since),
            Some(_) => {}
            None => self.open.clear(),
        }
        (self.open.len(), std::mem::take(&mut self.new))
    }
}

static NEEDS: Mutex<Needs> = Mutex::new(Needs::new());

/// What makes two needs the same: their words, whatever the case or spacing.
fn words(text: &str) -> String {
    text.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

/// The `need_you` tool: `caller` is the Henchman's pane, checked already.
pub(crate) fn need_you(state: &AppState, caller: &str, text: &str, task: Option<&str>) -> Result<Value> {
    let text = cut(text.trim(), SAYS);
    if text.is_empty() {
        return Err(Error::Other("say what you need the user for".into()));
    }
    let key = task.map(|task| {
        state
            .config
            .with(|c| {
                c.tasks
                    .iter()
                    .find(|t| t.id == task || t.issue_key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(task)))
                    .map(|t| t.issue_key.clone().unwrap_or_else(|| t.name.clone()))
            })
            .unwrap_or_else(|| task.to_string())
    });
    let need = Need { pane_id: caller.to_string(), key, text };
    NEEDS.lock().raise(need, Instant::now(), Utc::now())?;
    Ok(json!({ "told": true, "note": "Don't raise it again: it waits until the user opens your conversation." }))
}

/// The needs to banner now, and how many wait for the user (HEN-13).
pub fn henchman_needs(state: &AppState, panes: &[(PaneInfo, bool)]) -> (usize, Vec<Need>) {
    let henchman = panes.iter().map(|(p, _)| p).find(|p| p.running && is_henchman(state, p));
    NEEDS.lock().take(henchman)
}

/// The panes as attention sees them (HEN-13): while a Henchman runs, the
/// agents it hears of neither ping nor count when they finish or exit, since
/// it handles them and reaches the user itself. A permission question, a
/// usage limit or a trust prompt still rings: only the user answers those.
pub fn henchman_quiet<'a>(state: &AppState, panes: &'a [(PaneInfo, bool)]) -> Cow<'a, [(PaneInfo, bool)]> {
    if !panes.iter().any(|(p, _)| p.running && is_henchman(state, p)) {
        return Cow::Borrowed(panes);
    }
    let heard = Heard::new(state, true);
    Cow::Owned(
        panes
            .iter()
            .map(|(p, stopping)| {
                if p.kind == PaneKind::Agent && heard.hears(state, p) {
                    hushed(p, *stopping)
                } else {
                    (p.clone(), *stopping)
                }
            })
            .collect(),
    )
}

/// One the Henchman hears of: a finished turn reads as looked at, and an
/// exit as stopped on purpose, so neither is news.
fn hushed(p: &PaneInfo, stopping: bool) -> (PaneInfo, bool) {
    let mut p = p.clone();
    if p.activity == Activity::Done && p.notice.is_none() {
        p.activity = Activity::Idle;
    }
    let stopping = stopping || !p.running;
    (p, stopping)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(activity: Activity) -> PaneInfo {
        let now = Utc::now();
        PaneInfo {
            id: "p1".into(),
            task_id: "t1".into(),
            checkout_id: None,
            kind: PaneKind::Agent,
            title: "Claude Code".into(),
            agent_id: Some("claude".into()),
            cwd: "/tmp".into(),
            running: true,
            exit_code: None,
            started_at: now,
            last_output_at: now,
            notice: None,
            activity,
            activity_since: now,
            topic: None,
            acp: true,
            loop_said: None,
        }
    }

    fn need(text: &str) -> Need {
        Need { pane_id: "h".into(), key: Some("ACME-12".into()), text: text.into() }
    }

    #[test]
    fn a_finished_or_exited_agent_the_henchman_hears_is_not_news_but_a_question_still_is() {
        assert_eq!(hushed(&pane(Activity::Done), false).0.activity, Activity::Idle);
        assert_eq!(hushed(&pane(Activity::Asking), false).0.activity, Activity::Asking);
        let mut limited = pane(Activity::Done);
        limited.notice = Some("usage_limit".into());
        assert_eq!(hushed(&limited, false).0.activity, Activity::Done, "a usage limit is the user's");
        let mut gone = pane(Activity::Idle);
        gone.running = false;
        assert!(hushed(&gone, false).1, "an exit reads as stopped on purpose");
        assert!(!hushed(&pane(Activity::Working), false).1);
    }

    #[test]
    fn a_need_is_bannered_once_and_counted_until_the_user_opens_the_conversation() {
        let mut needs = Needs::default();
        let raised = Utc::now();
        assert!(needs.raise(need("needs the staging password"), Instant::now(), raised).is_ok());
        let mut working = pane(Activity::Working);
        working.activity_since = raised - chrono::Duration::seconds(5);
        let (open, new) = needs.take(Some(&working));
        assert_eq!((open, new.len()), (1, 1));
        assert_eq!(new[0].banner().1, "ACME-12: needs the staging password");
        let mut unseen = pane(Activity::Done);
        unseen.activity_since = raised + chrono::Duration::seconds(5);
        assert_eq!(needs.take(Some(&unseen)), (1, Vec::new()), "finished and not looked at");
        let mut seen = unseen.clone();
        seen.activity = Activity::Idle;
        assert_eq!(needs.take(Some(&seen)).0, 0);
    }

    #[test]
    fn a_conversation_looked_at_before_the_need_does_not_close_it() {
        let mut needs = Needs::default();
        let raised = Utc::now();
        assert!(needs.raise(need("pick one"), Instant::now(), raised).is_ok());
        let mut earlier = pane(Activity::Idle);
        earlier.activity_since = raised - chrono::Duration::seconds(30);
        assert_eq!(needs.take(Some(&earlier)).0, 1);
        assert_eq!(needs.take(None).0, 0, "with no Henchman, nobody holds it open");
    }

    #[test]
    fn the_same_need_is_not_raised_twice_and_a_flood_is_cut_off() {
        let mut needs = Needs::default();
        let now = Instant::now();
        assert!(needs.raise(need("Merge #34?"), now, Utc::now()).is_ok());
        assert!(needs.raise(need("merge   #34?"), now, Utc::now()).is_err(), "the same words");
        for n in 1..PER_HOUR {
            assert!(needs.raise(need(&format!("thing {n}")), now, Utc::now()).is_ok());
        }
        assert!(needs.raise(need("one more"), now, Utc::now()).is_err());
    }
}
