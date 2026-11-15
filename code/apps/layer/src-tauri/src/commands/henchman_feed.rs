//! What the Henchman is told as it happens (HEN-9, HEN-10): every agent
//! and chat that finishes a turn, was put back mid-turn after the app quit,
//! or exited by itself, batched into one message while it is busy, with its
//! standing orders, so it can answer what you've answered before, tell an
//! agent with steps of its own plan left to go on, and restart one that
//! fell over. Run from the attention pass (`attention.rs`), which already
//! looks at every pane.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::acp::Body;
use crate::config::StandingOrder;
use crate::pty::{Activity, PaneInfo, PaneKind};

use super::henchman::is_henchman;
use super::henchman_orders::key_of;
use super::henchman_tools::{cut, last_turn, pane_name};
use super::panes::CHAT_TASK_ID;
use super::AppState;

/// How much of what an agent last said goes into the message.
const SAID: usize = 800;
/// Past this many waiting to be told, the oldest go: `work_overview`
/// shows the rest, and a message of fifty is one nobody reads.
const QUEUE: usize = 12;
/// How long a pane put back mid-turn waits for a Henchman to hear of it.
/// Woken later than this, the user has likely seen to it themselves.
const STALE: Duration = Duration::from_secs(10 * 60);

/// Panes put back at launch that were mid-turn when the app quit, by their
/// new ids (HEN-10). Filled by the restore, which runs apart from the
/// attention pass that empties it.
static INTERRUPTED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// A pane the restore put back that was mid-turn at quit.
pub(crate) fn interrupted(pane_id: &str) {
    INTERRUPTED.lock().push((pane_id.to_string(), Instant::now()));
}

/// The agents in the middle of a turn, at a permission question included:
/// what the quit marks in their saved panes (HEN-10).
pub(crate) fn mid_turn(panes: &[(PaneInfo, bool)]) -> HashSet<String> {
    panes
        .iter()
        .map(|(p, _)| p)
        .filter(|p| {
            p.kind == PaneKind::Agent && p.running && matches!(p.activity, Activity::Working | Activity::Asking)
        })
        .map(|p| p.id.clone())
        .collect()
}

/// At quit, before the agents are stopped: which saved panes were in the
/// middle of a turn, for the restore to say so (HEN-10).
pub fn mark_mid_turn(state: &AppState) {
    let busy = mid_turn(&state.ptys.attention());
    let _ = state.config.update(|c| {
        for p in c.saved_panes.iter_mut() {
            p.mid_turn = busy.contains(&p.id);
        }
    });
}

/// Why it is in the message.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Why {
    Finished,
    /// Put back after the app quit in the middle of its turn.
    Interrupted,
    /// Its process ended without anyone stopping it.
    Exited,
}

/// One agent or chat that stopped.
#[derive(Debug, Clone, PartialEq)]
struct Stopped {
    pane_id: String,
    name: String,
    why: Why,
    said: Option<String>,
    asks: Option<String>,
    /// The error its turn ended on: overloaded, a lost connection.
    error: Option<String>,
}

/// The panes seen at the last pass, and what is still to be told.
#[derive(Default)]
pub struct HenchmanFeed {
    /// None until the first pass, which only records: what had finished
    /// before is in the brief.
    finished: Option<HashSet<String>>,
    /// Agents running at the last pass, to see which then exited.
    running: HashSet<String>,
    queue: Vec<Stopped>,
}

impl HenchmanFeed {
    pub fn pass(&mut self, state: &AppState, panes: &[(PaneInfo, bool)]) {
        let henchman = panes.iter().map(|(p, _)| p).find(|p| p.running && is_henchman(state, p));
        let (alone, threads) = state.config.with(|c| {
            let threads: HashSet<String> = c.slack_threads.iter().map(|t| t.room.clone()).collect();
            (c.henchman.left_alone.clone(), threads)
        });
        let heard = |p: &PaneInfo| {
            henchman.is_some()
                && !is_henchman(state, p)
                && !alone.iter().any(|k| k == key_of(p))
                // A Slack thread's chat answers to its thread (§24).
                && !threads.contains(&p.cwd)
        };
        let back = take_interrupted(henchman.is_some());
        let exited = self.newly_exited(panes, heard);
        let finished = self.newly_finished(panes.iter().map(|(p, _)| p), heard);
        for p in panes.iter().map(|(p, _)| p).filter(|p| p.running && back.contains(&p.id) && heard(p)) {
            self.queue.push(stopped(state, p, Why::Interrupted));
        }
        for p in exited {
            self.queue.push(stopped(state, p, Why::Exited));
        }
        for p in finished.into_iter().filter(|p| !back.contains(&p.id)) {
            self.queue.push(stopped(state, p, Why::Finished));
        }

        let Some(henchman) = henchman else {
            self.queue.clear();
            return;
        };
        // Itself put back mid-turn: it goes on with its own work first.
        if back.contains(&henchman.id) {
            let _ = state.ptys.submit(&henchman.id, RESTORED);
        }
        if self.queue.is_empty() || busy(state, henchman) {
            return;
        }
        let at = self.queue.len().saturating_sub(QUEUE);
        let told = &self.queue[at..];
        let orders = state.config.with(|c| c.henchman.orders.clone());
        let text = message(&orders, told, at);
        // Kept for the next pass if it can't be sent now.
        if state.ptys.submit(&henchman.id, &text).is_ok() {
            self.queue.clear();
        }
    }

    /// The panes that finished a turn since the last pass and that it hears
    /// of. A finished turn the user has looked at reads as idle, so one
    /// they answer themselves is never news.
    fn newly_finished<'a>(
        &mut self,
        panes: impl Iterator<Item = &'a PaneInfo>,
        heard: impl Fn(&PaneInfo) -> bool,
    ) -> Vec<&'a PaneInfo> {
        let mut now = HashSet::new();
        let mut new = Vec::new();
        for p in panes {
            if !finished(p) {
                continue;
            }
            now.insert(p.id.clone());
            if self.finished.as_ref().is_some_and(|seen| !seen.contains(&p.id)) && heard(p) {
                new.push(p);
            }
        }
        self.finished = Some(now);
        new
    }

    /// The agents that were running at the last pass and have since ended
    /// on their own: not stopped, closed, restarted or quit by the user, and
    /// for a terminal not by `/exit`, which ends it with 0. Over ACP the
    /// process ending is never the conversation's own doing.
    fn newly_exited<'a>(
        &mut self,
        panes: &'a [(PaneInfo, bool)],
        heard: impl Fn(&PaneInfo) -> bool,
    ) -> Vec<&'a PaneInfo> {
        let was = std::mem::take(&mut self.running);
        let mut new = Vec::new();
        for (p, stopping) in panes {
            if p.kind != PaneKind::Agent {
                continue;
            }
            if p.running {
                self.running.insert(p.id.clone());
            } else if was.contains(&p.id) && !stopping && (p.acp || p.exit_code != Some(0)) && heard(p) {
                new.push(p);
            }
        }
        new
    }
}

/// What the restore put back mid-turn, once a Henchman runs to hear it;
/// nothing while none does, and nothing stale.
fn take_interrupted(henchman: bool) -> HashSet<String> {
    let mut list = INTERRUPTED.lock();
    list.retain(|(_, at)| at.elapsed() < STALE);
    if !henchman {
        return HashSet::new();
    }
    std::mem::take(&mut *list).into_iter().map(|(id, _)| id).collect()
}

/// What the Henchman is told when it was itself put back mid-turn.
const RESTORED: &str =
    "[villain-layer · restored] Layer quit in the middle of your turn, and you are back. Go on where you were.";

/// Finished a turn and waiting: not at a permission question or a notice,
/// which only the user answers (HEN-9).
fn finished(p: &PaneInfo) -> bool {
    p.kind == PaneKind::Agent && p.running && p.activity == Activity::Done && p.notice.is_none()
}

fn busy(state: &AppState, henchman: &PaneInfo) -> bool {
    matches!(henchman.activity, Activity::Working | Activity::Asking)
        || state.ptys.acp_read(&henchman.id, None).is_ok_and(|v| v.busy)
}

fn stopped(state: &AppState, p: &PaneInfo, why: Why) -> Stopped {
    let name = if p.task_id == CHAT_TASK_ID {
        format!("Chat · {}", pane_name(p))
    } else {
        let task = state.config.task(&p.task_id).ok();
        let task = task.map(|t| t.issue_key.unwrap_or(t.name)).unwrap_or_else(|| "a task".into());
        format!("{task} · {}", pane_name(p))
    };
    let (said, asks, error) = match p.acp.then(|| state.ptys.acp_read(&p.id, None).ok()).flatten() {
        Some(view) => {
            let bodies = || view.changes.entries.iter().map(|e| &e.body);
            let (said, asks) = last_turn(bodies());
            (Some(cut(&said, SAID)), asks.map(|a| a.to_string()), failed_on(bodies()))
        }
        None => (None, None, None),
    };
    // A loop that handed its agent back says how it ended (LOOP-8).
    let said = p.loop_said.clone().map(|l| format!("(its loop: {l})")).or(said);
    Stopped { pane_id: p.id.clone(), name, why, said, asks, error }
}

/// The error the last turn ended on, if it did: what the app noted after
/// the user's last prompt.
fn failed_on<'a>(bodies: impl IntoIterator<Item = &'a Body>) -> Option<String> {
    let mut error = None;
    for body in bodies {
        match body {
            Body::User { dropped: false, .. } => error = None,
            Body::Note { text, error: true } => error = Some(cut(text, 300)),
            _ => {}
        }
    }
    error
}

/// The one message for everything that stopped since it was last told:
/// its first line names them, since that is all the conversation shows
/// of it until it is opened (ACP-19).
fn message(orders: &[StandingOrder], stopped: &[Stopped], dropped: usize) -> String {
    let mut m = format!("[villain-layer · {}] {}\n\n", tag(stopped), headline(stopped, dropped));
    for s in stopped {
        let why = match s.why {
            Why::Finished => "",
            Why::Interrupted => " — put back after Layer quit in the middle of its turn",
            Why::Exited => " — exited by itself",
        };
        m.push_str(&format!("- {} (pane {}){why}\n", s.name, s.pane_id));
        match &s.said {
            Some(said) if !said.is_empty() => m.push_str(&format!("  said: {}\n", said.replace('\n', "\n  "))),
            Some(_) => {}
            None => m.push_str("  a terminal: pane_read shows what it said\n"),
        }
        if let Some(asks) = &s.asks {
            m.push_str(&format!("  asks: {asks}\n"));
        }
        if let Some(error) = &s.error {
            m.push_str(&format!("  its turn ended on an error: {error}\n"));
        }
    }
    if dropped > 0 {
        m.push_str(&format!("- and {dropped} more before these: work_overview has them\n"));
    }
    m.push_str("\nYour standing orders:\n");
    if orders.is_empty() {
        m.push_str("none yet.\n");
    }
    for o in orders {
        m.push_str(&format!("- [{}] {}\n", o.id, o.text));
    }
    m.push_str(concat!(
        "\nFor each: if it asks something an order answers, answer it with pane_tell as the order ",
        "says; if it asks nothing and stopped with steps of its own plan left, tell it \"go on\". ",
        "One put back after Layer quit, or whose turn ended on an error a retry fixes (overloaded, ",
        "a rate limit, a lost connection), you tell to go on where it was. One that exited by ",
        "itself you restart with pane_restart, then tell it to go on; if it exits again, leave it ",
        "and tell me. Then one line here for each you answered (\"ACME-12: said go on\"). Anything ",
        "else, and anything you're unsure of, one line for me, unanswered. Finished work is mine ",
        "to look at: say it's done."
    ));
    m
}

/// The bracket's word: what they have in common.
fn tag(stopped: &[Stopped]) -> &'static str {
    let all = |why: Why| !stopped.is_empty() && stopped.iter().all(|s| s.why == why);
    if all(Why::Interrupted) {
        "restored"
    } else if all(Why::Exited) {
        "exited"
    } else if all(Why::Finished) {
        "finished"
    } else {
        "stopped"
    }
}

/// "ACME-12 · Claude Code and Chat · Why is staging slow stopped", or how
/// many past three.
fn headline(stopped: &[Stopped], dropped: usize) -> String {
    let n = stopped.len() + dropped;
    let names: Vec<&str> = stopped.iter().map(|s| s.name.as_str()).collect();
    let who = match names.as_slice() {
        _ if n > 3 => format!("{n} agents and chats"),
        [] => "Nothing".to_string(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let what = match tag(stopped) {
        "restored" => "were put back mid-turn",
        "exited" => "exited by themselves",
        "finished" => "finished a turn",
        _ => "stopped",
    };
    // "were" and "themselves" read for one as well as many here.
    format!("{who} {what}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, activity: Activity) -> PaneInfo {
        let now = chrono::Utc::now();
        PaneInfo {
            id: id.into(),
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
            acp: false,
            loop_said: None,
        }
    }

    fn ids(found: Vec<&PaneInfo>) -> Vec<&str> {
        found.into_iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn what_had_finished_before_the_first_look_is_not_news() {
        let mut feed = HenchmanFeed::default();
        let panes = [pane("a", Activity::Done)];
        assert!(feed.newly_finished(panes.iter(), |_| true).is_empty());
        assert!(feed.newly_finished(panes.iter(), |_| true).is_empty(), "nor the same turn again");
    }

    #[test]
    fn each_turn_an_agent_finishes_is_heard_once() {
        let mut feed = HenchmanFeed::default();
        feed.newly_finished([pane("a", Activity::Working)].iter(), |_| true);
        let done = [pane("a", Activity::Done)];
        assert_eq!(ids(feed.newly_finished(done.iter(), |_| true)), ["a"]);
        feed.newly_finished([pane("a", Activity::Working)].iter(), |_| true);
        assert_eq!(ids(feed.newly_finished(done.iter(), |_| true)), ["a"], "told to go on, it finished again");
    }

    #[test]
    fn a_permission_question_a_notice_or_a_turn_already_looked_at_is_not_heard() {
        let mut feed = HenchmanFeed::default();
        feed.newly_finished(std::iter::empty(), |_| true);
        let mut limited = pane("n", Activity::Done);
        limited.notice = Some("usage_limit".into());
        let mut shell = pane("s", Activity::Done);
        shell.kind = PaneKind::Shell;
        let panes = [pane("q", Activity::Asking), pane("i", Activity::Idle), limited, shell];
        assert!(feed.newly_finished(panes.iter(), |_| true).is_empty());
    }

    #[test]
    fn a_pane_left_alone_is_not_heard() {
        let mut feed = HenchmanFeed::default();
        feed.newly_finished(std::iter::empty(), |_| true);
        let panes = [pane("a", Activity::Done), pane("b", Activity::Done)];
        assert_eq!(ids(feed.newly_finished(panes.iter(), |p| p.id != "a")), ["b"]);
    }

    #[test]
    fn the_message_lists_what_finished_then_the_orders_then_what_to_do() {
        let orders = [StandingOrder { id: "o1".into(), text: "yes to running the tests".into() }];
        let stopped = [
            Stopped {
                pane_id: "p1".into(),
                name: "ACME-12 · Claude Code".into(),
                why: Why::Finished,
                said: Some("Done with step 2 of 4.".into()),
                asks: None,
                error: None,
            },
            Stopped {
                pane_id: "p2".into(),
                name: "ACME-14 · Codex".into(),
                why: Why::Finished,
                said: None,
                asks: None,
                error: None,
            },
        ];
        let m = message(&orders, &stopped, 3);
        let at = |s: &str| m.find(s).unwrap_or_else(|| panic!("{s} missing from:\n{m}"));
        assert!(at("ACME-12 · Claude Code (pane p1)") < at("said: Done with step 2 of 4."));
        assert!(at("ACME-14 · Codex (pane p2)") < at("pane_read shows"));
        assert!(at("and 3 more") < at("[o1] yes to running the tests"));
        assert!(at("[o1]") < at("go on"));
    }

    fn stop(name: &str, why: Why) -> Stopped {
        Stopped { pane_id: "p".into(), name: name.into(), why, said: Some(String::new()), asks: None, error: None }
    }

    #[test]
    fn the_first_line_names_what_stopped_since_it_is_all_the_conversation_shows() {
        let two = [stop("ACME-12 · Claude Code", Why::Finished), stop("Chat · Why so slow", Why::Finished)];
        let m = message(&[], &two, 0);
        assert!(
            m.starts_with("[villain-layer · finished] ACME-12 · Claude Code and Chat · Why so slow finished a turn.\n")
        );
        let back = [stop("ACME-12 · Claude Code", Why::Interrupted)];
        assert!(message(&[], &back, 0).starts_with("[villain-layer · restored] ACME-12 · Claude Code were put back"));
        let mixed = [stop("A", Why::Exited), stop("B", Why::Finished)];
        assert!(message(&[], &mixed, 2).starts_with("[villain-layer · stopped] 4 agents and chats stopped."));
    }

    #[test]
    fn an_agent_put_back_or_exited_says_so_with_the_error_its_turn_ended_on() {
        let mut exited = stop("ACME-9 · Claude Code", Why::Exited);
        exited.error = Some("Overloaded".into());
        let m = message(&[], &[exited, stop("ACME-3 · Codex", Why::Interrupted)], 0);
        assert!(m.contains("ACME-9 · Claude Code (pane p) — exited by itself\n"));
        assert!(m.contains("its turn ended on an error: Overloaded"));
        assert!(m.contains("— put back after Layer quit in the middle of its turn"));
        assert!(m.contains("pane_restart"));
    }

    #[test]
    fn an_agent_that_ends_on_its_own_is_heard_once_and_one_stopped_or_exited_is_not() {
        let mut feed = HenchmanFeed::default();
        let running = [
            (pane("crashed", Activity::Working), false),
            (pane("stopped", Activity::Working), false),
            (pane("exit", Activity::Done), false),
            (pane("acp", Activity::Done), false),
        ];
        assert!(feed.newly_exited(&running, |_| true).is_empty());
        let gone = |id: &str, code: Option<i32>, acp: bool, stopping: bool| {
            let mut p = pane(id, Activity::Idle);
            p.running = false;
            p.exit_code = code;
            p.acp = acp;
            (p, stopping)
        };
        let ended = [
            gone("crashed", Some(1), false, false),
            gone("stopped", Some(143), false, true),
            gone("exit", Some(0), false, false),
            gone("acp", Some(0), true, false),
        ];
        assert_eq!(ids(feed.newly_exited(&ended, |_| true)), ["crashed", "acp"]);
        assert!(feed.newly_exited(&ended, |_| true).is_empty(), "and only once");
    }

    #[test]
    fn a_turn_that_ended_on_an_error_says_which_and_a_new_prompt_clears_it() {
        let user = |text: &str| Body::User { text: text.into(), queued: false, dropped: false };
        let err = Body::Note { text: "API Error: Overloaded".into(), error: true };
        assert_eq!(failed_on([&user("go"), &err]).as_deref(), Some("API Error: Overloaded"));
        assert_eq!(failed_on([&user("go"), &err, &user("go on")]), None);
        let note = Body::Note { text: "This is a new one.".into(), error: false };
        assert_eq!(failed_on([&user("go"), &note]), None);
    }

    #[test]
    fn the_quit_marks_agents_working_or_asking_and_nothing_else() {
        let mut shell = pane("shell", Activity::Working);
        shell.kind = PaneKind::Shell;
        let mut exited = pane("exited", Activity::Working);
        exited.running = false;
        let panes = [
            (pane("working", Activity::Working), false),
            (pane("asking", Activity::Asking), false),
            (pane("done", Activity::Done), false),
            (shell, false),
            (exited, false),
        ];
        let mut marked: Vec<String> = mid_turn(&panes).into_iter().collect();
        marked.sort();
        assert_eq!(marked, ["asking", "working"]);
    }

    #[test]
    fn without_orders_it_says_there_are_none() {
        assert!(message(&[], &[], 0).contains("none yet."));
    }
}
