//! Routines (§29): the window's commands, and starting and watching runs
//! from the attention pass. When a routine is due and what is kept of it
//! is `crate::routines`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::agents;
use crate::config::HenchmanAgent;
use crate::error::{Error, Result};
use crate::messages::{self, Kind, Level, New};
use crate::pty::{Activity, PaneInfo};
use crate::routines::{self, Every, Routine, Routines, Run, RunState, View};
use crate::target::Target;

use super::acp_panes::{self, Start};
use super::henchman_tools::{cut, last_turn};
use super::panes::{Resume, CHAT_TASK_ID};
use super::AppState;

/// At launch, routines wait this long, so the agents Layer puts back start
/// first (ROUT-6, LOOP-11).
const HOLD: Duration = Duration::from_secs(60);
/// How much of what a run said its row shows.
const SAID: usize = 160;

/// Where every run's room is: `_chat/routines/<routine>/<run>` (ROUT-4).
fn rooms(state: &AppState) -> PathBuf {
    state.config.worktree_root().join("_chat").join("routines")
}

/// Whether a pane's folder is a routine run's. By path, not by creating it:
/// this is asked of every pane in every pass.
pub(crate) fn is_run_room(state: &AppState, cwd: &str) -> bool {
    Path::new(cwd).starts_with(rooms(state))
}

/// The agent a run in `cwd` was started with, to open it on its routine's
/// model and effort (ROUT-4); none for any other folder.
pub(crate) fn run_agent(app: &AppHandle, state: &AppState, cwd: &str) -> Option<HenchmanAgent> {
    let routine = Path::new(cwd).strip_prefix(rooms(state)).ok()?.components().next()?;
    let id = routine.as_os_str().to_string_lossy().to_string();
    routines::read(app, state.config.folder(), |all| all.routines.iter().find(|r| r.id == id).map(|r| r.agent.clone()))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// Open a run's conversation in `room`: new with the routine's prompt, or
/// picked up where it ended (ROUT-9).
fn open(app: &AppHandle, state: &AppState, routine: &Routine, room: &Path, resume: Resume) -> Result<PaneInfo> {
    let id = &routine.agent.agent_id;
    let def = agents::find(id)
        .filter(|d| d.acp.is_some())
        .ok_or_else(|| Error::NotFound(format!("{id}, an agent that runs as a conversation")))?;
    std::fs::create_dir_all(room)?;
    // Best effort, as for any chat: no context file is no reason not to run.
    let _ = super::panes::write_chat_context(state, room);
    let _ = crate::mcp::write_config(room);
    let start = Start {
        task_id: CHAT_TASK_ID.to_string(),
        checkout_id: None,
        cwd: room.to_string_lossy().to_string(),
        title: routine.name.clone(),
        prompt: (resume == Resume::No).then(|| routine.prompt.clone()),
        resume,
    };
    acp_panes::start(app, state, def, start)
}

/// Start a run of routine `id`. One that cannot start is recorded as
/// failed, with why (ROUT-7); the finished runs before it are closed
/// (ROUT-9).
pub(crate) fn start(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    due: Option<DateTime<Utc>>,
    caught_up: bool,
) -> Result<Run> {
    let folder = state.config.folder().to_path_buf();
    let (routine, live) = routines::read(app, &folder, |all| {
        (all.routines.iter().find(|r| r.id == id).cloned(), all.live_run(id).is_some())
    });
    let routine = routine.ok_or_else(|| Error::NotFound(format!("routine {id}")))?;
    if live {
        return Err(Error::Other(format!("{} is still running: its run is at the top of the list.", routine.name)));
    }
    if routine.prompt.trim().is_empty() {
        return Err(Error::Other(format!("Write what {} asks first.", routine.name)));
    }
    let run_id = new_id();
    let room = rooms(state).join(&routine.id).join(&run_id);
    let mut run = Run {
        id: run_id,
        routine_id: routine.id.clone(),
        due,
        started: Utc::now(),
        caught_up,
        room: room.to_string_lossy().to_string(),
        pane_id: None,
        session: None,
        state: RunState::Working,
        worked: false,
        said: String::new(),
        ended: None,
    };
    match open(app, state, &routine, &room, Resume::No) {
        Ok(pane) => {
            run.pane_id = Some(pane.id.clone());
            let _ = app.emit("pty:activity", &pane.id);
        }
        Err(e) => {
            run.state = RunState::Failed;
            run.said = e.to_string();
            run.ended = Some(Utc::now());
        }
    }
    let finished = routines::change(app, &folder, |all| {
        let mut finished = Vec::new();
        for r in all.runs.iter_mut().filter(|r| r.routine_id == routine.id && !r.state.live()) {
            finished.extend(r.pane_id.take());
        }
        all.add_run(run.clone());
        (finished, true)
    });
    close_later(app, finished);
    Ok(run)
}

/// Close panes off the caller's thread: stopping an agent waits for it to
/// save, and the attention pass and the window must not.
fn close_later(app: &AppHandle, panes: Vec<String>) {
    if panes.is_empty() {
        return;
    }
    let app = app.clone();
    let close = move || {
        let state = app.state::<AppState>();
        for id in panes {
            let _ = state.ptys.close(&id);
            let _ = app.emit("pty:activity", &id);
        }
    };
    if let Err(e) = std::thread::Builder::new().name("routine-close".into()).spawn(close) {
        eprintln!("a routine's finished runs could not be closed: {e}");
    }
}

/// Every routine and every run kept (ROUT-1, ROUT-8).
#[tauri::command]
pub async fn routines_list(app: AppHandle) -> Result<View> {
    let handle = app.clone();
    super::blocking(app, move |state| Ok(routines::read(&handle, state.config.folder(), Routines::view))).await
}

/// A new routine, on, every day at 08:00, with the Henchman's agent; it
/// waits for its first time (ROUT-5). Its id.
#[tauri::command]
pub async fn routine_create(app: AppHandle) -> Result<String> {
    let handle = app.clone();
    super::blocking(app, move |state| create(&handle, state, RoutinePatch::default())).await
}

/// A new routine with `fields` over those defaults, refused whole when one
/// is wrong (ROUT-2, ROUT-11). Its id.
pub(crate) fn create(app: &AppHandle, state: &AppState, fields: RoutinePatch) -> Result<String> {
    check_agent(&fields)?;
    let agent = state.config.with(|c| c.henchman.agent.clone());
    let id = new_id();
    let mut routine = Routine { id: id.clone(), name: "New routine".into(), agent, ..Default::default() };
    routines::settle(&mut routine, &Local::now());
    apply(&mut routine, fields, &Local::now())?;
    routines::change(app, state.config.folder(), |all| {
        all.routines.push(routine);
        ((), true)
    });
    Ok(id)
}

/// What changed on a routine's page; a field left out stays.
#[derive(Debug, Default, Deserialize)]
pub struct RoutinePatch {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub every: Option<Every>,
    pub at: Option<String>,
    pub weekday: Option<u8>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub on: Option<bool>,
}

/// Change a routine (ROUT-2). Another time, or turning it on, waits for
/// the next time rather than running for one already past (ROUT-5).
#[tauri::command]
pub async fn routine_update(app: AppHandle, id: String, patch: RoutinePatch) -> Result<()> {
    let handle = app.clone();
    super::blocking(app, move |state| update(&handle, state, &id, patch)).await
}

pub(crate) fn update(app: &AppHandle, state: &AppState, id: &str, patch: RoutinePatch) -> Result<()> {
    check_agent(&patch)?;
    routines::change(app, state.config.folder(), |all| {
        let Some(r) = all.routine_mut(id) else {
            return (Err(Error::NotFound(format!("routine {id}"))), false);
        };
        let out = apply(r, patch, &Local::now());
        let ok = out.is_ok();
        (out, ok)
    })
}

fn check_agent(patch: &RoutinePatch) -> Result<()> {
    if let Some(agent) = &patch.agent_id {
        agents::find(agent)
            .filter(|d| d.acp.is_some())
            .ok_or_else(|| Error::Other(format!("{agent} does not run as a conversation (ACP).")))?;
    }
    Ok(())
}

fn apply(r: &mut Routine, patch: RoutinePatch, now: &DateTime<Local>) -> Result<()> {
    // Every field is checked before any is changed: a refused change
    // leaves the routine as it was.
    let name = patch.name.as_deref().map(str::trim);
    if name.is_some_and(str::is_empty) {
        return Err(Error::Other("A routine needs a name.".into()));
    }
    if patch.at.as_deref().is_some_and(|at| routines::parse_at(at).is_none()) {
        return Err(Error::Other("A time is hours and minutes, as 08:00.".into()));
    }
    if patch.weekday.is_some_and(|d| d > 6) {
        return Err(Error::Other("A week has seven days.".into()));
    }
    if let Some(name) = name {
        r.name = name.to_string();
    }
    if let Some(prompt) = patch.prompt {
        r.prompt = prompt;
    }
    let was = (r.every, r.at.clone(), r.weekday, r.on);
    r.every = patch.every.unwrap_or(r.every);
    r.at = patch.at.map(|a| a.trim().to_string()).unwrap_or_else(|| r.at.clone());
    r.weekday = patch.weekday.unwrap_or(r.weekday);
    r.on = patch.on.unwrap_or(r.on);
    if let Some(agent) = patch.agent_id {
        // Another agent's models and efforts are its own: back to it deciding.
        if agent != r.agent.agent_id {
            r.agent.model.clear();
            r.agent.effort.clear();
        }
        r.agent.agent_id = agent;
    }
    if let Some(model) = patch.model {
        r.agent.model = model;
    }
    if let Some(effort) = patch.effort {
        r.agent.effort = effort;
    }
    if was != (r.every, r.at.clone(), r.weekday, r.on) {
        routines::settle(r, now);
    }
    Ok(())
}

/// Delete a routine and its runs, stopping one still at work (ROUT-2).
#[tauri::command]
pub async fn routine_delete(app: AppHandle, id: String) -> Result<()> {
    let handle = app.clone();
    super::blocking(app, move |state| delete(&handle, state, &id)).await
}

pub(crate) fn delete(app: &AppHandle, state: &AppState, id: &str) -> Result<()> {
    let gone = routines::change(app, state.config.folder(), |all| {
        if all.routine_mut(id).is_none() {
            return (None, false);
        }
        let panes: Vec<String> =
            all.runs.iter().filter(|r| r.routine_id == id).filter_map(|r| r.pane_id.clone()).collect();
        all.routines.retain(|r| r.id != id);
        all.runs.retain(|r| r.routine_id != id);
        (Some(panes), true)
    });
    let panes = gone.ok_or_else(|| Error::NotFound(format!("routine {id}")))?;
    for pane in panes {
        let _ = state.ptys.close(&pane);
        let _ = app.emit("pty:activity", &pane);
    }
    // Its id is one Layer made, so this is a folder of its own.
    let _ = std::fs::remove_dir_all(rooms(state).join(id));
    Ok(())
}

/// Run a routine now, whatever its schedule (ROUT-5).
#[tauri::command]
pub async fn routine_run_now(app: AppHandle, id: String) -> Result<Run> {
    let handle = app.clone();
    super::blocking(app, move |state| start(&handle, state, &id, None, false)).await
}

/// A run's conversation, opened again on it where its pane has gone
/// (ROUT-9).
#[tauri::command]
pub async fn routine_open_run(app: AppHandle, run_id: String) -> Result<PaneInfo> {
    let handle = app.clone();
    super::blocking(app, move |state| {
        let folder = state.config.folder().to_path_buf();
        let (run, routine) = routines::read(&handle, &folder, |all| {
            let run = all.runs.iter().find(|r| r.id == run_id).cloned();
            let routine = run.as_ref().and_then(|run| all.routines.iter().find(|r| r.id == run.routine_id).cloned());
            (run, routine)
        });
        let (run, routine) = run.zip(routine).ok_or_else(|| Error::NotFound(format!("run {run_id}")))?;
        if let Some(pane) = &run.pane_id {
            match state.ptys.info(pane) {
                Ok(info) if info.running => return Ok(info),
                Ok(_) => {
                    let _ = state.ptys.close(pane);
                }
                Err(_) => {}
            }
        }
        if run.room.is_empty() {
            return Err(Error::Other("This run never started, so it has no conversation.".into()));
        }
        let resume = Resume::Restart { cwd: run.room.clone(), session: run.session.clone() };
        let pane = open(&handle, state, &routine, Path::new(&run.room), resume)?;
        routines::change(&handle, &folder, |all| match all.run_mut(&run_id) {
            Some(r) => {
                r.pane_id = Some(pane.id.clone());
                ((), true)
            }
            None => ((), false),
        });
        let _ = handle.emit("pty:activity", &pane.id);
        Ok(pane)
    })
    .await
}

/// What the attention pass keeps between passes for routines.
pub struct RoutineWatch {
    began: Instant,
}

impl Default for RoutineWatch {
    fn default() -> Self {
        Self { began: Instant::now() }
    }
}

/// What a run's pane says it is doing, read before the routines are locked.
struct Seen {
    running: bool,
    activity: Activity,
    said: Option<String>,
    session: Option<String>,
}

/// How a live run stands now, and the news it makes: `Some(asking)` when it
/// starts asking (true) or finishes (false).
fn advance(run: &mut Run, seen: Option<&Seen>, now: DateTime<Utc>) -> Option<bool> {
    if run.session.is_none() {
        run.session = seen.and_then(|s| s.session.clone());
    }
    let Some(seen) = seen.filter(|s| s.running) else {
        run.state = RunState::Stopped;
        run.ended = Some(now);
        if run.said.is_empty() {
            run.said = "Its agent stopped before it finished.".into();
        }
        return None;
    };
    match seen.activity {
        Activity::Working => {
            run.worked = true;
            run.state = RunState::Working;
            None
        }
        Activity::Asking => {
            let news = run.state != RunState::Asking;
            run.state = RunState::Asking;
            news.then_some(true)
        }
        Activity::Done => finish(run, seen, now),
        Activity::Idle if run.worked => finish(run, seen, now),
        Activity::Idle => None,
    }
}

fn finish(run: &mut Run, seen: &Seen, now: DateTime<Utc>) -> Option<bool> {
    run.state = RunState::Done;
    run.ended = Some(now);
    run.said = seen.said.clone().unwrap_or_default();
    Some(false)
}

/// The first line of what an agent said, without its Markdown marks.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| l.trim().trim_start_matches(['#', '>', '-', '*', ' ']).trim_end_matches(['*', ' ']))
        .find(|l| !l.is_empty())
        .unwrap_or("");
    cut(line, SAID)
}

impl RoutineWatch {
    /// One pass: watch the runs at work, and start the routines that are
    /// due (ROUT-5 to ROUT-7, ROUT-10). `runs` are the panes in run rooms.
    pub fn pass(&mut self, app: &AppHandle, state: &AppState, runs: &[(PaneInfo, bool)], notify: bool) {
        let folder = state.config.folder().to_path_buf();
        let seen: HashMap<&str, Seen> = runs
            .iter()
            .map(|(p, _)| {
                let ended = matches!(p.activity, Activity::Done | Activity::Idle);
                let said = ended
                    .then(|| state.ptys.acp_read(&p.id, None).ok())
                    .flatten()
                    .map(|v| first_line(&last_turn(v.changes.entries.iter().map(|e| &e.body)).0));
                let seen = Seen {
                    running: p.running,
                    activity: p.activity,
                    said,
                    session: state.ptys.session(&p.id).ok().flatten(),
                };
                (p.id.as_str(), seen)
            })
            .collect();
        let now = Utc::now();
        let news = routines::change(app, &folder, |all| {
            let names: HashMap<String, String> = all.routines.iter().map(|r| (r.id.clone(), r.name.clone())).collect();
            let mut news = Vec::new();
            let mut changed = false;
            for run in all.runs.iter_mut().filter(|r| r.state.live()) {
                let before = run.clone();
                let seen = run.pane_id.as_deref().and_then(|p| seen.get(p));
                if let Some(asking) = advance(run, seen, now) {
                    let name = names.get(&run.routine_id).cloned().unwrap_or_default();
                    news.push((name, run.routine_id.clone(), asking, run.said.clone()));
                }
                changed |= *run != before;
            }
            (news, changed)
        });
        if !news.is_empty() {
            tell(app, &news, notify);
        }
        if self.began.elapsed() < HOLD {
            return;
        }
        self.start_due(app, state, &folder);
    }

    fn start_due(&self, app: &AppHandle, state: &AppState, folder: &Path) {
        let now = Local::now();
        let due = routines::change(app, folder, |all| {
            let mut due = Vec::new();
            let mut skipped = Vec::new();
            let mut changed = false;
            let live: Vec<String> = all.runs.iter().filter(|r| r.state.live()).map(|r| r.routine_id.clone()).collect();
            for r in all.routines.iter_mut() {
                let before = r.last_due;
                let Some((at, late)) = routines::take_due(r, &now) else {
                    changed |= r.last_due != before;
                    continue;
                };
                changed = true;
                if r.prompt.trim().is_empty() {
                    continue;
                }
                if live.contains(&r.id) {
                    skipped.push(skip(r, at));
                } else {
                    due.push((r.id.clone(), at, late));
                }
            }
            for run in skipped {
                all.add_run(run);
            }
            (due, changed)
        });
        for (id, at, late) in due {
            if let Err(e) = start(app, state, &id, Some(at), late) {
                eprintln!("routine {id} did not start: {e}");
            }
        }
    }
}

/// A time passed while the run before was still at it (ROUT-7).
fn skip(r: &Routine, at: DateTime<Utc>) -> Run {
    let now = Utc::now();
    Run {
        id: new_id(),
        routine_id: r.id.clone(),
        due: Some(at),
        started: now,
        caught_up: false,
        room: String::new(),
        pane_id: None,
        session: None,
        state: RunState::Skipped,
        worked: false,
        said: "The run before was still at work.".into(),
        ended: Some(now),
    }
}

/// Into the message center always, as banners while the window is away and
/// the switch is on (ROUT-10). Each opens its routine.
fn tell(app: &AppHandle, news: &[(String, String, bool, String)], notify: bool) {
    let lines: Vec<(String, String, String, Level)> = news
        .iter()
        .map(|(name, routine, asking, said)| {
            let body = match (asking, said.is_empty()) {
                (true, _) => "is asking for your permission".to_string(),
                (false, true) => "has finished".to_string(),
                (false, false) => said.clone(),
            };
            let level = if *asking { Level::Info } else { Level::Success };
            (name.clone(), body, Target::Routine(routine).to_string(), level)
        })
        .collect();
    messages::record_all(
        app,
        lines
            .iter()
            .map(|(title, body, target, level)| New {
                kind: Kind::Agent,
                level: *level,
                title: title.clone(),
                body: body.clone(),
                target: Some(target.clone()),
            })
            .collect(),
    );
    if notify {
        for (title, body, target, _) in lines {
            let _ = super::banner(app, title, body, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run {
            id: "a".into(),
            routine_id: "r1".into(),
            due: None,
            started: Utc::now(),
            caught_up: false,
            room: "/x".into(),
            pane_id: Some("p1".into()),
            session: None,
            state: RunState::Working,
            worked: false,
            said: String::new(),
            ended: None,
        }
    }

    fn seen(activity: Activity) -> Seen {
        Seen { running: true, activity, said: Some("Nothing needs you.".into()), session: Some("s1".into()) }
    }

    #[test]
    fn a_run_quiet_before_it_has_worked_is_still_starting() {
        let mut r = run();
        assert_eq!(advance(&mut r, Some(&seen(Activity::Idle)), Utc::now()), None);
        assert_eq!(r.state, RunState::Working);
        assert_eq!(r.session.as_deref(), Some("s1"), "its conversation is kept as soon as it has one");
    }

    #[test]
    fn a_run_that_worked_and_went_quiet_is_done_with_what_it_said() {
        let mut r = run();
        advance(&mut r, Some(&seen(Activity::Working)), Utc::now());
        assert_eq!(advance(&mut r, Some(&seen(Activity::Idle)), Utc::now()), Some(false));
        assert_eq!(r.state, RunState::Done);
        assert_eq!(r.said, "Nothing needs you.");
        assert!(r.ended.is_some());
    }

    #[test]
    fn a_run_that_asks_is_news_once() {
        let mut r = run();
        assert_eq!(advance(&mut r, Some(&seen(Activity::Asking)), Utc::now()), Some(true));
        assert_eq!(advance(&mut r, Some(&seen(Activity::Asking)), Utc::now()), None);
        assert_eq!(r.state, RunState::Asking);
    }

    #[test]
    fn a_run_whose_pane_has_gone_is_stopped() {
        let mut r = run();
        assert_eq!(advance(&mut r, None, Utc::now()), None);
        assert_eq!(r.state, RunState::Stopped);
        assert!(!r.said.is_empty());
    }

    #[test]
    fn another_time_waits_for_the_next_one_but_a_new_prompt_changes_nothing_else() {
        let now = Local::now();
        let mut r = Routine { id: "r1".into(), last_due: None, ..Default::default() };
        apply(&mut r, RoutinePatch { prompt: Some("Brief me.".into()), ..Default::default() }, &now).unwrap();
        assert_eq!(r.last_due, None, "the schedule did not change");
        apply(&mut r, RoutinePatch { every: Some(Every::Hourly), ..Default::default() }, &now).unwrap();
        assert!(r.last_due.is_some());
        assert!(routines::take_due(&mut r, &now).is_none(), "no run for a time already past");
    }

    #[test]
    fn choosing_another_agent_leaves_its_model_and_effort_to_it() {
        let now = Local::now();
        let mut r = Routine::default();
        apply(&mut r, RoutinePatch { agent_id: Some("copilot".into()), ..Default::default() }, &now).unwrap();
        assert_eq!((r.agent.model.as_str(), r.agent.effort.as_str()), ("", ""));
    }

    #[test]
    fn a_bad_time_or_an_empty_name_is_refused() {
        let now = Local::now();
        let mut r = Routine::default();
        assert!(apply(&mut r, RoutinePatch { at: Some("8 am".into()), ..Default::default() }, &now).is_err());
        assert!(apply(&mut r, RoutinePatch { name: Some("  ".into()), ..Default::default() }, &now).is_err());
        assert_eq!(r.at, "08:00");
    }

    #[test]
    fn a_runs_row_shows_the_first_line_it_said_without_markdown() {
        assert_eq!(first_line("\n## **3 need you**\n- LAY-212"), "3 need you");
    }
}
