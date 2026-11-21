//! Routines (§29): prompts asked on a schedule, each run a new chat. This
//! is when a routine is due and what is kept of it, in `routines.json`
//! beside the config (ROUT-3); `commands/routines.rs` starts and watches
//! the runs.
//!
//! Times are the Mac's own clock: "weekdays at 08:00" means 08:00 where the
//! user is, through a change to summer time too.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

use crate::config::HenchmanAgent;

/// Runs kept per routine (ROUT-8).
const RUNS_KEPT: usize = 30;
/// A run started this long after its time caught up (ROUT-6): Layer was
/// closed or the Mac asleep. Under it, the pass was only a few seconds late.
const LATE_AFTER: chrono::Duration = chrono::Duration::minutes(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Every {
    Hourly,
    #[default]
    Daily,
    Weekdays,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub every: Every,
    /// "HH:MM"; hourly reads only the minutes.
    pub at: String,
    /// Weekly's day, 0 for Monday to 6 for Sunday.
    pub weekday: u8,
    /// Who runs it, on what model and effort, as for the Henchman (HEN-15).
    pub agent: HenchmanAgent,
    pub on: bool,
    /// The newest time it was due that has been dealt with: run, skipped,
    /// or passed while it was off or being made (ROUT-5).
    pub last_due: Option<DateTime<Utc>>,
}

impl Default for Routine {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            prompt: String::new(),
            every: Every::Daily,
            at: "08:00".into(),
            weekday: 0,
            agent: HenchmanAgent::default(),
            on: true,
            last_due: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Working,
    Asking,
    Done,
    Stopped,
    Skipped,
    Failed,
}

impl RunState {
    /// Still at it: a new time is skipped (ROUT-7) and its pane stays.
    pub fn live(self) -> bool {
        matches!(self, RunState::Working | RunState::Asking)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub routine_id: String,
    /// The time it was due at; none for Run now.
    #[serde(default)]
    pub due: Option<DateTime<Utc>>,
    pub started: DateTime<Utc>,
    #[serde(default)]
    pub caught_up: bool,
    /// Its chat's folder, which its conversation is kept by (CHAT-1).
    #[serde(default)]
    pub room: String,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    pub state: RunState,
    /// It has been seen working, so going quiet is finishing, not starting.
    #[serde(default)]
    pub worked: bool,
    /// The first line of what its agent last said, or why it did not run.
    #[serde(default)]
    pub said: String,
    #[serde(default)]
    pub ended: Option<DateTime<Utc>>,
}

/// What `routines.json` holds.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routines {
    #[serde(default)]
    pub routines: Vec<Routine>,
    /// Newest first.
    #[serde(default)]
    pub runs: Vec<Run>,
}

/// A routine as the window shows it: with when it runs next.
#[derive(Debug, Clone, Serialize)]
pub struct RoutineView {
    #[serde(flatten)]
    pub routine: Routine,
    pub next: Option<DateTime<Utc>>,
}

/// What the window is sent: every routine and every run kept.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub routines: Vec<RoutineView>,
    pub runs: Vec<Run>,
}

impl Routines {
    pub fn view(&self) -> View {
        let now = Local::now();
        let routines = self
            .routines
            .iter()
            .map(|r| RoutineView {
                routine: r.clone(),
                next: r.on.then(|| next_due(r, &now)).flatten().map(|t| t.with_timezone(&Utc)),
            })
            .collect();
        View { routines, runs: self.runs.clone() }
    }

    pub fn routine_mut(&mut self, id: &str) -> Option<&mut Routine> {
        self.routines.iter_mut().find(|r| r.id == id)
    }

    pub fn run_mut(&mut self, id: &str) -> Option<&mut Run> {
        self.runs.iter_mut().find(|r| r.id == id)
    }

    /// The routine's run still at work, if any (ROUT-7).
    pub fn live_run(&self, routine: &str) -> Option<&Run> {
        self.runs.iter().find(|r| r.routine_id == routine && r.state.live())
    }

    /// Record a run, newest first, keeping only the newest of its routine's.
    pub fn add_run(&mut self, run: Run) {
        let routine = run.routine_id.clone();
        self.runs.insert(0, run);
        let mut seen = 0;
        self.runs.retain(|r| {
            if r.routine_id != routine {
                return true;
            }
            seen += 1;
            seen <= RUNS_KEPT
        });
    }

    /// Runs left at work by a Layer that quit: nothing watches them now,
    /// and they are not put back at launch (ROUT-4, ROUT-9).
    fn stop_left_over(&mut self) {
        let now = Utc::now();
        for run in self.runs.iter_mut().filter(|r| r.state.live()) {
            run.state = RunState::Stopped;
            run.pane_id = None;
            run.ended.get_or_insert(now);
            if run.said.is_empty() {
                run.said = "Layer was quit while it worked.".into();
            }
        }
    }
}

/// The hour and minute of "HH:MM", if it is one.
pub fn parse_at(at: &str) -> Option<(u32, u32)> {
    let (h, m) = at.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60 && !at.trim().is_empty()).then_some((h, m))
}

/// Its times on one day, earliest first.
fn times_on(r: &Routine, day: NaiveDate) -> Vec<NaiveDateTime> {
    let (hour, minute) = parse_at(&r.at).unwrap_or((8, 0));
    let at = |h: u32| day.and_hms_opt(h, minute, 0);
    let workday = !matches!(day.weekday(), Weekday::Sat | Weekday::Sun);
    match r.every {
        Every::Hourly => (0..24).filter_map(at).collect(),
        Every::Daily => at(hour).into_iter().collect(),
        Every::Weekdays if workday => at(hour).into_iter().collect(),
        Every::Weekly if day.weekday().num_days_from_monday() == u32::from(r.weekday.min(6)) => {
            at(hour).into_iter().collect()
        }
        Every::Weekdays | Every::Weekly => Vec::new(),
    }
}

/// The latest time it was due at or before `now`. A time that does not
/// exist on the day the clocks go forward is passed over.
pub fn latest_due<Tz: TimeZone>(r: &Routine, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let tz = now.timezone();
    let today = now.naive_local().date();
    (0..=8).filter_map(|back| today.checked_sub_days(Days::new(back))).find_map(|day| {
        times_on(r, day).into_iter().rev().filter_map(|t| tz.from_local_datetime(&t).earliest()).find(|t| t <= now)
    })
}

/// The first time it is due after `now`.
pub fn next_due<Tz: TimeZone>(r: &Routine, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    let tz = now.timezone();
    let today = now.naive_local().date();
    (0..=8).filter_map(|ahead| today.checked_add_days(Days::new(ahead))).find_map(|day| {
        times_on(r, day).into_iter().filter_map(|t| tz.from_local_datetime(&t).earliest()).find(|t| t > now)
    })
}

/// A routine made, turned on or given another time waits for its next
/// time: the ones already past are dealt with (ROUT-5).
pub fn settle<Tz: TimeZone>(r: &mut Routine, now: &DateTime<Tz>) {
    r.last_due = latest_due(r, now).map(|t| t.with_timezone(&Utc));
}

/// Whether it is due now: the time it was due at, and whether that was a
/// while ago (ROUT-6). However many times passed, it is due once, and that
/// time is then dealt with.
pub fn take_due<Tz: TimeZone>(r: &mut Routine, now: &DateTime<Tz>) -> Option<(DateTime<Utc>, bool)> {
    if !r.on {
        return None;
    }
    let due = latest_due(r, now)?.with_timezone(&Utc);
    let Some(last) = r.last_due else {
        // Never settled (a hand-edited file): from now on, not at once.
        r.last_due = Some(due);
        return None;
    };
    if due <= last {
        return None;
    }
    r.last_due = Some(due);
    Some((due, now.with_timezone(&Utc) - due > LATE_AFTER))
}

struct Kept {
    path: PathBuf,
    routines: Routines,
}

impl Kept {
    /// A missing file is no routines. One that cannot be read is kept
    /// beside it, since the next write replaces it.
    fn load(dir: &Path) -> Self {
        let path = dir.join("routines.json");
        let mut routines = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                let kept = path.with_extension("json.unreadable");
                let _ = std::fs::copy(&path, &kept);
                eprintln!("routines.json could not be read ({e}); the old one is at {}", kept.display());
                Routines::default()
            }),
            Err(_) => Routines::default(),
        };
        routines.stop_left_over();
        Kept { path, routines }
    }

    /// Written off the caller's thread, as the lair is: changes come from
    /// the attention pass too. A write overtaken by a newer one skips.
    fn save(&self) {
        let seq = WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        let path = self.path.clone();
        let bytes = serde_json::to_vec_pretty(&self.routines);
        let write = move || {
            let _disk = DISK.lock();
            if WRITES.load(Ordering::SeqCst) != seq {
                return;
            }
            if let Err(e) = bytes.map_err(Into::into).and_then(|b| crate::config::write_whole(&path, &b)) {
                eprintln!("routines.json could not be written: {e}");
            }
        };
        if let Err(e) = std::thread::Builder::new().name("routines".into()).spawn(write) {
            eprintln!("routines.json could not be written: {e}");
        }
    }
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
static WRITES: AtomicU64 = AtomicU64::new(0);
static DISK: Mutex<()> = Mutex::new(());

/// Change the routines; `f` says whether it did. A change is written and
/// sent to the window, whole, as `routines:changed`. Nothing inside `f`
/// may start or stop a pane: the lock is held.
pub fn change<R: Runtime, T>(app: &AppHandle<R>, folder: &Path, f: impl FnOnce(&mut Routines) -> (T, bool)) -> T {
    let (out, changed) = {
        let mut kept = KEPT.lock();
        let kept = kept.get_or_insert_with(|| Kept::load(folder));
        let (out, changed) = f(&mut kept.routines);
        if changed {
            kept.save();
        }
        (out, changed.then(|| kept.routines.view()))
    };
    if let Some(view) = changed {
        let _ = app.emit("routines:changed", view);
    }
    out
}

/// Read them without changing anything.
pub fn read<R: Runtime, T>(app: &AppHandle<R>, folder: &Path, f: impl FnOnce(&Routines) -> T) -> T {
    change(app, folder, |r| (f(r), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn routine(every: Every, time: &str) -> Routine {
        Routine { id: "r1".into(), every, at: time.into(), ..Default::default() }
    }

    #[test]
    fn a_daily_routine_was_last_due_today_once_its_time_has_passed_and_yesterday_before() {
        let r = routine(Every::Daily, "08:00");
        assert_eq!(latest_due(&r, &at("2026-11-20T09:12:00+01:00")), Some(at("2026-11-20T08:00:00+01:00")));
        assert_eq!(latest_due(&r, &at("2026-11-20T07:59:00+01:00")), Some(at("2026-11-19T08:00:00+01:00")));
        assert_eq!(next_due(&r, &at("2026-11-20T08:00:00+01:00")), Some(at("2026-11-21T08:00:00+01:00")));
    }

    #[test]
    fn a_weekday_routine_passes_over_the_weekend() {
        let r = routine(Every::Weekdays, "08:00");
        // Monday 07:00: last due Friday, next due Monday 08:00.
        assert_eq!(latest_due(&r, &at("2026-11-23T07:00:00+01:00")), Some(at("2026-11-20T08:00:00+01:00")));
        assert_eq!(next_due(&r, &at("2026-11-21T10:00:00+01:00")), Some(at("2026-11-23T08:00:00+01:00")));
    }

    #[test]
    fn a_weekly_routine_runs_on_its_day_and_an_hourly_one_at_its_minute() {
        let weekly = Routine { weekday: 2, ..routine(Every::Weekly, "17:30") };
        // Wednesday 2026-11-18.
        assert_eq!(next_due(&weekly, &at("2026-11-20T10:00:00+01:00")), Some(at("2026-11-25T17:30:00+01:00")));
        assert_eq!(latest_due(&weekly, &at("2026-11-20T10:00:00+01:00")), Some(at("2026-11-18T17:30:00+01:00")));
        let hourly = routine(Every::Hourly, "09:15");
        assert_eq!(latest_due(&hourly, &at("2026-11-20T10:14:00+01:00")), Some(at("2026-11-20T09:15:00+01:00")));
        assert_eq!(next_due(&hourly, &at("2026-11-20T10:14:00+01:00")), Some(at("2026-11-20T10:15:00+01:00")));
    }

    #[test]
    fn a_routine_made_after_its_time_waits_for_the_next_one() {
        let mut r = routine(Every::Daily, "08:00");
        settle(&mut r, &at("2026-11-20T09:00:00+01:00"));
        assert_eq!(take_due(&mut r, &at("2026-11-20T09:00:05+01:00")), None);
        let (due, late) = take_due(&mut r, &at("2026-11-21T08:00:04+01:00")).unwrap();
        assert_eq!(due, at("2026-11-21T08:00:00+01:00"));
        assert!(!late, "a pass a few seconds after the time is on time");
        assert_eq!(take_due(&mut r, &at("2026-11-21T08:00:09+01:00")), None, "it runs once for its time");
    }

    #[test]
    fn times_missed_while_layer_was_closed_run_once_and_say_they_caught_up() {
        let mut r = routine(Every::Hourly, "00:00");
        settle(&mut r, &at("2026-11-20T09:30:00+01:00"));
        // Closed from 09:30 until 13:10: four times missed, one run.
        let (due, late) = take_due(&mut r, &at("2026-11-20T13:10:00+01:00")).unwrap();
        assert_eq!(due, at("2026-11-20T13:00:00+01:00"));
        assert!(late);
        assert_eq!(take_due(&mut r, &at("2026-11-20T13:10:05+01:00")), None);
    }

    #[test]
    fn a_routine_that_is_off_never_comes_due() {
        let mut r = Routine { on: false, ..routine(Every::Hourly, "00:00") };
        settle(&mut r, &at("2026-11-20T09:30:00+01:00"));
        assert_eq!(take_due(&mut r, &at("2026-11-20T13:10:00+01:00")), None);
    }

    #[test]
    fn only_a_routines_newest_runs_are_kept() {
        let mut all = Routines::default();
        let run = |id: usize, routine: &str| Run {
            id: id.to_string(),
            routine_id: routine.into(),
            due: None,
            started: Utc::now(),
            caught_up: false,
            room: String::new(),
            pane_id: None,
            session: None,
            state: RunState::Done,
            worked: true,
            said: String::new(),
            ended: None,
        };
        all.add_run(run(0, "other"));
        for i in 1..=RUNS_KEPT + 5 {
            all.add_run(run(i, "r1"));
        }
        assert_eq!(all.runs.iter().filter(|r| r.routine_id == "r1").count(), RUNS_KEPT);
        assert_eq!(all.runs[0].id, (RUNS_KEPT + 5).to_string(), "newest first");
        assert!(all.runs.iter().any(|r| r.routine_id == "other"), "another routine's are left alone");
    }

    #[test]
    fn runs_left_at_work_by_a_quit_are_stopped_when_layer_opens_again() {
        let dir = std::env::temp_dir().join(format!("vl-routines-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let left = Routines {
            routines: Vec::new(),
            runs: vec![Run {
                id: "a".into(),
                routine_id: "r1".into(),
                due: None,
                started: Utc::now(),
                caught_up: false,
                room: "/x".into(),
                pane_id: Some("p1".into()),
                session: Some("s".into()),
                state: RunState::Working,
                worked: true,
                said: String::new(),
                ended: None,
            }],
        };
        std::fs::write(dir.join("routines.json"), serde_json::to_vec(&left).unwrap()).unwrap();
        let kept = Kept::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kept.routines.runs[0].state, RunState::Stopped);
        assert_eq!(kept.routines.runs[0].pane_id, None);
        assert_eq!(kept.routines.runs[0].session.as_deref(), Some("s"), "its conversation can still be opened");
    }

    #[test]
    fn a_time_must_be_hours_and_minutes() {
        assert_eq!(parse_at("08:05"), Some((8, 5)));
        assert_eq!(parse_at("24:00"), None);
        assert_eq!(parse_at("8"), None);
        assert_eq!(parse_at("ab:cd"), None);
    }
}
