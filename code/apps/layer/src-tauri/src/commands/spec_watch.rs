//! Follows the spec folder of the task whose Spec tab is on screen (§19), so
//! what an agent writes there shows as it is written.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};

use crate::error::{Error, Result};

use super::spec_copy::spec_dir;

/// The app's watcher.
static WATCHER: SpecWatcher = SpecWatcher::new();

/// Follow the spec folder of `task_id`, the task whose Spec tab is on screen,
/// and say `spec:changed` when something in it changes (SPEC-15). `None`, or
/// a task with no spec to show, follows nothing. One task at a time.
#[tauri::command]
pub async fn watch_spec(app: AppHandle, task_id: Option<String>) -> Result<()> {
    let sink = app.clone();
    super::blocking(app, move |state| {
        let target = match task_id {
            Some(id) => match state.config.task(&id) {
                Ok(task) => Some((id, spec_dir(state, &task))),
                Err(e) => {
                    WATCHER.watch(None, |_| {})?;
                    return Err(e);
                }
            },
            None => None,
        };
        WATCHER.watch(target, move |id| drop(sink.emit("spec:changed", id)))
    })
    .await
}

/// How long the folder must be quiet before it is reported. An agent saving a
/// file writes it in a few steps, and a tool that writes five files writes
/// them one after another.
const QUIET: Duration = Duration::from_millis(200);

/// A folder that never goes quiet is reported this often at the least.
const MOST_WAIT: Duration = Duration::from_secs(1);

/// The one watcher: it follows one task's folder at a time.
pub(crate) struct SpecWatcher {
    running: Mutex<Option<Running>>,
}

/// What the thread of a followed folder is told.
enum Msg {
    Event(notify::Result<Event>),
    Stop,
}

/// A followed folder. Dropping it stops the thread, which drops the watcher.
struct Running {
    tx: Sender<Msg>,
}

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.tx.send(Msg::Stop));
    }
}

impl SpecWatcher {
    pub(crate) const fn new() -> Self {
        Self { running: Mutex::new(None) }
    }

    /// Follow `target` (a task id and its spec folder) instead of whatever was
    /// followed before; `None` follows nothing. `notice` gets the task id once
    /// a burst of changes has settled.
    ///
    /// A folder that is not there yet is followed through its nearest folder
    /// that is, and from the moment it appears, itself: an agent's first write
    /// makes the folder, and that is when the tab has something to show.
    pub(crate) fn watch(
        &self,
        target: Option<(String, PathBuf)>,
        notice: impl FnMut(&str) + Send + 'static,
    ) -> Result<()> {
        let mut running = self.running.lock();
        // Stopped first, so a failure to start leaves nothing followed.
        *running = None;
        let Some((id, dir)) = target else { return Ok(()) };

        let (tx, rx) = channel();
        let handler = tx.clone();
        let watcher = notify::recommended_watcher(move |e| drop(handler.send(Msg::Event(e)))).map_err(refused)?;
        let mut follow = Follow { id, dir: resolve(&dir), anchor: PathBuf::new(), watcher };
        follow.point().map_err(refused)?;

        std::thread::spawn(move || follow.run(&rx, notice));
        *running = Some(Running { tx });
        Ok(())
    }
}

fn refused(e: notify::Error) -> Error {
    Error::Other(format!("cannot follow the spec folder: {e}"))
}

/// One followed folder, on the thread that reports it.
struct Follow {
    id: String,
    /// The folder, by the name the system reports its files under.
    dir: PathBuf,
    /// What the watcher is on now: `dir`, or the nearest folder above it that
    /// exists.
    anchor: PathBuf,
    watcher: RecommendedWatcher,
}

impl Follow {
    /// Report each burst of changes once it has gone quiet, until stopped.
    fn run(mut self, rx: &Receiver<Msg>, mut notice: impl FnMut(&str)) {
        loop {
            // Asleep until something changes in the folder.
            loop {
                match rx.recv() {
                    Ok(Msg::Event(e)) if self.concerns(&e) => break,
                    Ok(Msg::Event(_)) => {}
                    Ok(Msg::Stop) | Err(_) => return,
                }
            }
            let first = Instant::now();
            let mut until = first + QUIET;
            loop {
                match rx.recv_timeout(until.min(first + MOST_WAIT).saturating_duration_since(Instant::now())) {
                    Ok(Msg::Event(e)) => {
                        if self.concerns(&e) {
                            until = Instant::now() + QUIET;
                        }
                    }
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            notice(&self.id);
        }
    }

    /// Whether `event` changed something in the folder, or made or removed the
    /// folder itself. Looking at a file is not a change.
    fn concerns(&mut self, event: &notify::Result<Event>) -> bool {
        let Ok(event) = event else { return false };
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        let touched = event.paths.iter().any(|p| p.starts_with(&self.dir));
        // Moved first: it must happen for every event, a folder that appeared
        // between two of them included.
        self.point().unwrap_or(false) || touched
    }

    /// Put the watcher where it should be. True when it moved.
    fn point(&mut self) -> notify::Result<bool> {
        let anchor = nearest(&self.dir);
        if anchor == self.anchor {
            return Ok(false);
        }
        drop(self.watcher.unwatch(&self.anchor));
        let mode = if anchor == self.dir { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        self.watcher.watch(&anchor, mode)?;
        self.anchor = anchor;
        Ok(true)
    }
}

/// `dir` or, when it is not there, the nearest folder above it that is.
fn nearest(dir: &Path) -> PathBuf {
    dir.ancestors().find(|p| p.is_dir()).unwrap_or(dir).to_path_buf()
}

/// `dir` under the names the system reports: FSEvents says `/private/var`
/// where a temp folder is `/var`. What is not there yet keeps its own names.
fn resolve(dir: &Path) -> PathBuf {
    let there = nearest(dir);
    match (there.canonicalize(), dir.strip_prefix(&there)) {
        (Ok(real), Ok(rest)) => real.join(rest),
        _ => dir.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    const SETTLE: Duration = Duration::from_millis(1500);
    const QUIET: Duration = Duration::from_millis(700);

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vl-spec-watch-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Follow `dir` as task `id`, with notices on a channel. FSEvents may still
    /// be reporting the folder's own making, so what is already queued is let
    /// go before the test writes.
    fn follow(watcher: &SpecWatcher, id: &str, dir: &std::path::Path) -> Receiver<String> {
        let (tx, rx) = channel();
        watcher.watch(Some((id.into(), dir.to_path_buf())), move |id| drop(tx.send(id.to_string()))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        while rx.try_recv().is_ok() {}
        rx
    }

    #[test]
    fn a_write_in_the_spec_folder_is_reported_once_after_the_debounce() {
        let dir = temp("burst").join("spec");
        std::fs::create_dir_all(&dir).unwrap();
        let watcher = SpecWatcher::new();
        let rx = follow(&watcher, "t1", &dir);

        for n in 0..3 {
            std::fs::write(dir.join("tasks.md"), format!("- [ ] step {n}\n")).unwrap();
            std::thread::sleep(Duration::from_millis(15));
        }

        assert_eq!(rx.recv_timeout(SETTLE).as_deref(), Ok("t1"));
        assert!(rx.recv_timeout(QUIET).is_err(), "the burst was reported more than once");
    }

    #[test]
    fn watching_another_task_stops_the_first() {
        let (first, second) = (temp("first").join("spec"), temp("second").join("spec"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let watcher = SpecWatcher::new();
        let rx_first = follow(&watcher, "t1", &first);
        let rx_second = follow(&watcher, "t2", &second);

        std::fs::write(first.join("design.md"), "old\n").unwrap();
        std::fs::write(second.join("design.md"), "new\n").unwrap();

        assert_eq!(rx_second.recv_timeout(SETTLE).as_deref(), Ok("t2"));
        assert!(rx_first.recv_timeout(QUIET).is_err(), "the first task was still followed");
    }

    #[test]
    fn watching_nothing_stops_the_watcher() {
        let dir = temp("stop").join("spec");
        std::fs::create_dir_all(&dir).unwrap();
        let watcher = SpecWatcher::new();
        let rx = follow(&watcher, "t1", &dir);

        watcher.watch(None, |_| {}).unwrap();
        std::fs::write(dir.join("design.md"), "x\n").unwrap();

        assert!(rx.recv_timeout(QUIET).is_err());
    }

    #[test]
    fn the_first_write_shows_when_the_spec_folder_does_not_exist_yet() {
        let task = temp("late");
        let dir = task.join("spec");
        let watcher = SpecWatcher::new();
        let rx = follow(&watcher, "t1", &dir);

        std::fs::write(task.join("notes.txt"), "not the spec\n").unwrap();
        assert!(rx.recv_timeout(QUIET).is_err(), "a file outside spec/ was reported");

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("requirements.md"), "# Refunds\n").unwrap();
        assert_eq!(rx.recv_timeout(SETTLE).as_deref(), Ok("t1"));

        // From here on it follows the folder itself.
        std::fs::write(dir.join("design.md"), "x\n").unwrap();
        assert_eq!(rx.recv_timeout(SETTLE).as_deref(), Ok("t1"));
    }
}
