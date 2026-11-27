//! Keeping the Mac usable while agents work (PANE-20, PANE-21, PANE-22).
//!
//! On an 18 GB Mac, a dozen agents each running its repository's tests ran
//! 47 test workers of node, 8 `tsc` and a Docker VM at once: 35 GB of
//! programs. The screen and keyboard stopped answering, and the Mac had to be
//! switched off with its button, twice in a week. Nothing in the app noticed.

use std::ffi::c_void;
use std::time::Duration;

/// How much lower than everything else an agent, and all it starts, is
/// scheduled. It only matters when the cores are all busy: then the window
/// server, the keyboard and this app's window go first. Not the Mac's
/// background policy (`PRIO_DARWIN_BG`), which keeps a process on the
/// efficiency cores and throttles its disk even on an idle Mac: a test run
/// took several times longer for nothing.
pub(crate) const AGENT_NICE: i32 = 10;

/// Lower an agent's priority (PANE-20). Its process group too, which a pane
/// started as a terminal leads and an ACP agent leads with `process_group(0)`:
/// whatever it started before this ran is lowered with it, and whatever it
/// starts later inherits it.
pub(crate) fn lower_priority(pid: u32) {
    // SAFETY: setpriority takes plain integers and touches no memory of ours.
    // A failure (the process already gone) changes nothing worth reporting.
    unsafe {
        libc::setpriority(libc::PRIO_PGRP, pid, AGENT_NICE);
        libc::setpriority(libc::PRIO_PROCESS, pid, AGENT_NICE);
    }
}

/// What macOS says about its memory, as `memory_pressure` prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Pressure {
    Normal,
    Warn,
    Critical,
}

/// The pressure now. Read from the kernel, not by running `memory_pressure`:
/// it is asked before every spawn, some of them on the main thread.
pub(crate) fn pressure() -> Pressure {
    level(read_level())
}

fn read_level() -> Option<i32> {
    let mut level: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated, and `level` is a c_int whose size
    // is passed in `len`, which is what this sysctl writes.
    let r = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut level as *mut libc::c_int).cast::<c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0).then_some(level)
}

/// The kernel's level: 1 normal, 2 warning, 4 critical. Anything unreadable
/// counts as normal, so a Mac that will not say never stops agents.
fn level(raw: Option<i32>) -> Pressure {
    match raw {
        Some(n) if n >= 4 => Pressure::Critical,
        Some(n) if n >= 2 => Pressure::Warn,
        _ => Pressure::Normal,
    }
}

/// Where the pressure is read from: the kernel, or a test's own answer.
pub(crate) struct Gauge(pub(crate) fn() -> Pressure);

impl Default for Gauge {
    fn default() -> Self {
        // A test's panes must not start or not by how busy the Mac running
        // the tests is.
        if cfg!(test) {
            Self(|| Pressure::Normal)
        } else {
            Self(pressure)
        }
    }
}

/// How long a launch waits between the agents it puts back (PANE-21): long
/// enough for the memory the last one took to show in the pressure before
/// the next one starts.
pub(crate) const RESTORE_GAP: Duration = Duration::from_millis(1500);

/// Why an agent does not start now (PANE-21).
pub(crate) const SHORT_OF_MEMORY: &str = "macOS says this Mac is critically short of memory, so no \
     agent starts now: another one could freeze it. Stop some agents, or quit an app or a dev server, \
     then start it again. A terminal still opens.";

/// What every agent is told about sharing the Mac (PANE-22), in the task's
/// and the chat room's context file. None of the test runners reads a
/// number of workers from the environment (Vitest, Playwright and Jest
/// were checked), so the agent has to pass it.
pub(crate) const SHARED_MAC: &str = "## This Mac is shared\n\n\
     Several agents work on this Mac at once, and they share its memory and cores. Run tests and \
     builds with at most two workers: Playwright `--workers=2`, Vitest and Jest `--maxWorkers=2`, \
     pytest `-n 2`, Cargo `-j 2`, and the like for any other runner, whatever its config says. \
     Run the tests for what you changed rather than every suite, one run at a time, and stop the \
     dev servers and watchers you started once you are done with them.\n\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_levels_read_as_normal_warn_and_critical() {
        assert_eq!(level(Some(1)), Pressure::Normal);
        assert_eq!(level(Some(2)), Pressure::Warn);
        assert_eq!(level(Some(4)), Pressure::Critical);
    }

    #[test]
    fn a_mac_that_does_not_say_never_stops_agents() {
        assert_eq!(level(None), Pressure::Normal);
        assert_eq!(level(Some(0)), Pressure::Normal);
    }

    #[test]
    fn this_mac_answers_with_a_level() {
        assert!(read_level().is_some_and(|n| n >= 1));
    }

    #[test]
    fn a_lowered_agent_and_what_it_starts_run_at_the_lower_priority() {
        let mut child =
            std::process::Command::new("/bin/sh").args(["-c", "sleep 5 & wait"]).spawn().expect("sh starts");
        lower_priority(child.id());
        // SAFETY: getpriority reads an integer about a process.
        let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, child.id()) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(nice, AGENT_NICE);
    }
}
