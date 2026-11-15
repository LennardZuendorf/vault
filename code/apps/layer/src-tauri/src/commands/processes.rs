//! What Layer's panes started and still runs (HEN-6): a dev server, a
//! watcher, a build. Read from `ps` and `lsof`, since a process a shell
//! starts tells nobody it did.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{Error, Result};
use crate::pty::PaneInfo;

use super::panes::CHAT_TASK_ID;
use super::AppState;

/// One process a pane started.
#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    pub pane_id: String,
    pub task_id: String,
    /// Who started it: the task's ticket key or name, or the chat's name.
    pub owner: String,
    /// The program's name and its arguments.
    pub command: String,
    /// How long it has run, in seconds.
    pub secs: u64,
    /// The TCP ports it, or anything under it, listens on.
    pub ports: Vec<u16>,
}

/// A process started by a shell counts once it has run this long: a
/// command's quick `git status` is not worth a row.
const SETTLED_SECS: u64 = 5;

/// How long Stop waits for SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

const SHELLS: &[&str] = &["sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh"];

#[derive(Debug, Clone, PartialEq)]
struct Ps {
    pid: u32,
    ppid: u32,
    secs: u64,
    command: String,
}

impl Ps {
    /// The program's own name: `-zsh` (a login shell) is zsh.
    fn program(&self) -> &str {
        let first = self.command.split_whitespace().next().unwrap_or_default();
        first.rsplit('/').next().unwrap_or(first).trim_start_matches('-')
    }

    fn is_shell(&self) -> bool {
        SHELLS.contains(&self.program())
    }

    /// The program's name and its arguments, cut to a line.
    fn shown(&self) -> String {
        let rest = self.command.split_once(char::is_whitespace).map(|(_, r)| r.trim()).unwrap_or_default();
        let line = if rest.is_empty() { self.program().to_string() } else { format!("{} {rest}", self.program()) };
        match line.char_indices().nth(160) {
            Some((at, _)) => format!("{}…", &line[..at]),
            None => line,
        }
    }
}

/// `ps -axo pid=,ppid=,etime=,command=`, a process a line.
fn parse_ps(out: &str) -> Vec<Ps> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let secs = parse_etime(parts.next()?)?;
            let command = parts.collect::<Vec<_>>().join(" ");
            (!command.is_empty()).then_some(Ps { pid, ppid, secs, command })
        })
        .collect()
}

/// `ps`'s elapsed time, `[[dd-]hh:]mm:ss`, in seconds.
fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// `lsof -F pn`'s answer: each pid's listening ports.
fn parse_lsof(out: &str) -> HashMap<u32, Vec<u16>> {
    let mut ports: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in out.lines() {
        match line.split_at_checked(1) {
            Some(("p", rest)) => pid = rest.parse().ok(),
            Some(("n", name)) => {
                let port = name.rsplit(':').next().and_then(|p| p.parse::<u16>().ok());
                if let (Some(pid), Some(port)) = (pid, port) {
                    let list = ports.entry(pid).or_default();
                    if !list.contains(&port) {
                        list.push(port);
                    }
                }
            }
            _ => {}
        }
    }
    ports
}

/// Below each pane's own process, the top-most of each branch that counts:
/// not a shell, and either started by one and running `SETTLED_SECS`, or
/// listening on a port. Its children are its, not rows of their own.
fn pick(rows: &[Ps], roots: &[u32], listening: &HashMap<u32, Vec<u16>>) -> Vec<(u32, u32)> {
    let by_pid: HashMap<u32, &Ps> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for r in rows {
        children.entry(r.ppid).or_default().push(r.pid);
    }
    let counts = |p: &Ps| {
        let parent_shell = by_pid.get(&p.ppid).is_some_and(|parent| parent.is_shell());
        !p.is_shell() && ((parent_shell && p.secs >= SETTLED_SECS) || listening.contains_key(&p.pid))
    };
    let mut out = Vec::new();
    for &root in roots {
        let mut stack: Vec<u32> = children.get(&root).cloned().unwrap_or_default();
        while let Some(pid) = stack.pop() {
            let Some(p) = by_pid.get(&pid) else { continue };
            if counts(p) {
                out.push((root, pid));
            } else {
                stack.extend(children.get(&pid).into_iter().flatten());
            }
        }
    }
    out
}

/// A process and everything under it.
fn subtree(rows: &[Ps], top: u32) -> Vec<u32> {
    let mut out = vec![top];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        out.extend(rows.iter().filter(|r| r.ppid == parent).map(|r| r.pid));
        i += 1;
    }
    out
}

fn ps() -> Result<Vec<Ps>> {
    let out = Command::new("/bin/ps").args(["-axo", "pid=,ppid=,etime=,command="]).output()?;
    if !out.status.success() {
        return Err(Error::Other("ps could not list processes".into()));
    }
    Ok(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

/// The listening TCP ports of `pids`. `lsof` exits 1 when there are none.
fn listening(pids: &[u32]) -> HashMap<u32, Vec<u16>> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    match Command::new("/usr/sbin/lsof")
        .args(["-nP", "-w", "-a", "-iTCP", "-sTCP:LISTEN", "-p", &list, "-Fpn"])
        .output()
    {
        Ok(out) => parse_lsof(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => HashMap::new(),
    }
}

/// Who a pane works for, as a row says it.
fn owner(state: &AppState, pane: &PaneInfo) -> String {
    if pane.task_id == CHAT_TASK_ID {
        return pane.topic.clone().unwrap_or_else(|| pane.title.clone());
    }
    match state.config.task(&pane.task_id) {
        Ok(task) => task.issue_key.unwrap_or(task.name),
        Err(_) => pane.title.clone(),
    }
}

/// Every process Layer's panes started that still runs, newest first.
/// Blocking: it runs `ps` and `lsof`.
pub(crate) fn list(state: &AppState) -> Result<Vec<Process>> {
    let panes = state.ptys.running_pids();
    if panes.is_empty() {
        return Ok(Vec::new());
    }
    let rows = ps()?;
    let roots: Vec<u32> = panes.iter().map(|(_, pid)| *pid).collect();
    let under: Vec<u32> = roots.iter().flat_map(|r| subtree(&rows, *r)).filter(|p| !roots.contains(p)).collect();
    let ports = listening(&under);
    let by_pid: HashMap<u32, &Ps> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut out: Vec<Process> = pick(&rows, &roots, &ports)
        .into_iter()
        .filter_map(|(root, pid)| {
            let (pane, _) = panes.iter().find(|(_, p)| *p == root)?;
            let p = by_pid.get(&pid)?;
            let mut held: Vec<u16> =
                subtree(&rows, pid).iter().flat_map(|q| ports.get(q).cloned().unwrap_or_default()).collect();
            held.sort_unstable();
            held.dedup();
            Some(Process {
                pid,
                pane_id: pane.id.clone(),
                task_id: pane.task_id.clone(),
                owner: owner(state, pane),
                command: p.shown(),
                secs: p.secs,
                ports: held,
            })
        })
        .collect();
    out.sort_by_key(|p| p.secs);
    Ok(out)
}

#[tauri::command]
pub async fn running_processes(app: AppHandle) -> Result<Vec<Process>> {
    super::blocking(app, list).await
}

/// End a process a pane started, and everything under it: SIGTERM, then
/// SIGKILL to what is left after `GRACE`. Only while it is still one of
/// the list's, so a pid reused since cannot be hit.
#[tauri::command]
pub async fn stop_process(app: AppHandle, pid: u32) -> Result<Vec<Process>> {
    super::blocking(app, move |state| {
        if !list(state)?.iter().any(|p| p.pid == pid) {
            return Err(Error::Other("that process has ended, or no pane started it".into()));
        }
        let before = ps()?;
        let doomed: HashSet<(u32, String)> = subtree(&before, pid)
            .into_iter()
            .filter_map(|q| before.iter().find(|r| r.pid == q).map(|r| (r.pid, r.command.clone())))
            .collect();
        for (q, _) in &doomed {
            // SAFETY: kill(2) on a pid read from ps a moment ago.
            unsafe { libc::kill(*q as i32, libc::SIGTERM) };
        }
        std::thread::sleep(GRACE);
        for r in ps()? {
            if doomed.contains(&(r.pid, r.command.clone())) {
                // SAFETY: the same process, by pid and command line.
                unsafe { libc::kill(r.pid as i32, libc::SIGKILL) };
            }
        }
        list(state)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, secs: u64, command: &str) -> Ps {
        Ps { pid, ppid, secs, command: command.into() }
    }

    #[test]
    fn ps_lines_are_read_with_their_whole_command() {
        let out = "  101     1   01:02:03 /bin/zsh -l\n  202   101 2-00:00:05 bun run dev --port 1420\nnot a line\n";
        assert_eq!(
            parse_ps(out),
            vec![row(101, 1, 3723, "/bin/zsh -l"), row(202, 101, 172_805, "bun run dev --port 1420")]
        );
    }

    #[test]
    fn elapsed_times_of_every_length_are_read() {
        assert_eq!(parse_etime("00:07"), Some(7));
        assert_eq!(parse_etime("12:34"), Some(754));
        assert_eq!(parse_etime("01:00:00"), Some(3600));
        assert_eq!(parse_etime("3-00:00:01"), Some(259_201));
        assert_eq!(parse_etime("x"), None);
    }

    #[test]
    fn listening_ports_are_read_per_pid_from_every_address_form() {
        let out = "p300\nf12\nn*:5173\nf13\nn[::1]:5173\np400\nf5\nn127.0.0.1:7380\n";
        let ports = parse_lsof(out);
        assert_eq!(ports.get(&300), Some(&vec![5173]));
        assert_eq!(ports.get(&400), Some(&vec![7380]));
    }

    #[test]
    fn a_command_a_shell_started_is_listed_once_it_has_settled_and_its_children_are_its() {
        // claude (the pane) → zsh -c → bun run dev → node vite; and a quick git.
        let rows = vec![
            row(10, 1, 900, "claude"),
            row(11, 10, 600, "/bin/zsh -c bun run dev"),
            row(12, 11, 600, "bun run dev"),
            row(13, 12, 599, "node /x/vite.js"),
            row(14, 10, 1, "/bin/zsh -c git status"),
            row(15, 14, 1, "git status"),
        ];
        assert_eq!(pick(&rows, &[10], &HashMap::new()), vec![(10, 12)]);
    }

    #[test]
    fn something_listening_counts_however_new_and_whoever_started_it() {
        let rows = vec![row(10, 1, 900, "claude"), row(20, 10, 1, "node mcp-server.js")];
        let ports = HashMap::from([(20, vec![9000])]);
        assert_eq!(pick(&rows, &[10], &ports), vec![(10, 20)]);
        assert_eq!(pick(&rows, &[10], &HashMap::new()), vec![]);
    }

    #[test]
    fn the_panes_own_shell_and_agent_are_never_rows() {
        let rows = vec![row(10, 1, 900, "-zsh"), row(11, 10, 900, "/bin/bash")];
        assert_eq!(pick(&rows, &[10], &HashMap::new()), vec![]);
    }

    #[test]
    fn a_row_shows_the_programs_name_and_is_cut_to_a_line() {
        assert_eq!(row(1, 0, 0, "/usr/local/bin/bun run dev").shown(), "bun run dev");
        assert_eq!(row(1, 0, 0, "/usr/bin/make").shown(), "make");
        let long = format!("cargo {}", "x".repeat(300));
        assert!(row(1, 0, 0, &long).shown().ends_with('…'));
    }

    #[test]
    fn a_subtree_is_a_process_and_everything_under_it() {
        let rows = vec![row(1, 0, 0, "a"), row(2, 1, 0, "b"), row(3, 2, 0, "c"), row(4, 0, 0, "d")];
        assert_eq!(subtree(&rows, 1), vec![1, 2, 3]);
    }
}
