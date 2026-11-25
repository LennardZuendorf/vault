//! How an install reaches the outside (§19): a process to run and a git
//! clone to make. A trait, so that the install's tests run no `npx` and touch
//! no network; the real one is here, with its limits.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::git;

/// How an install reaches the outside: a process, and a git clone. A trait so
/// that the tests run no `npx` and touch no network.
pub(crate) trait Runner {
    /// Run `argv` (never a shell string) in `cwd`; what it printed. An error
    /// when it fails or outlives `timeout`.
    fn run(&self, argv: &[String], cwd: &Path, timeout: Duration) -> Result<String>;
    /// Clone `url` at `ref` into `into`, which does not exist yet.
    fn fetch(&self, url: &str, r#ref: &str, into: &Path) -> Result<()>;
}

/// The runner the app uses.
pub(crate) struct Real;

impl Runner for Real {
    fn run(&self, argv: &[String], cwd: &Path, timeout: Duration) -> Result<String> {
        let program = argv.first().filter(|p| !p.is_empty()).ok_or_else(|| Error::Other("nothing to run".into()))?;
        // A GUI app's PATH is /usr/bin:/bin: npx is found the way a terminal
        // would find it, or not at all.
        let found = if program.contains('/') { Some(program.clone()) } else { crate::shellenv::which(program) };
        let found = found.ok_or_else(|| Error::Other(format!("{program} is not installed, or not on your PATH")))?;
        let mut child = Command::new(found)
            .args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(crate::shellenv::user_env())
            // Nothing to type into: a step that asks fails instead of waiting.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| Error::Other(format!("could not start {program}: {e}")))?;
        let out = child.stdout.take().map(read_tail);
        let err = child.stderr.take().map(read_tail);
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                // SAFETY: a negative pid is the process group, which is the
                // step's own (process_group(0) above): what it started goes too.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let said = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
            String::from_utf8_lossy(&h.and_then(|h| h.join().ok()).unwrap_or_default()).to_string()
        };
        let printed = format!("{}{}", said(out), said(err));
        match status {
            None => Err(Error::Other(format!(
                "{} did not finish within {} seconds. {}",
                argv.join(" "),
                timeout.as_secs(),
                tail(&printed, 1024).trim()
            ))),
            Some(s) if s.success() => Ok(printed),
            Some(s) => Err(Error::Other(format!("{program} failed ({s}). {}", tail(&printed, 1024).trim()))),
        }
    }

    fn fetch(&self, url: &str, r#ref: &str, into: &Path) -> Result<()> {
        git::clone_at(url, r#ref, into)
    }
}

/// What a pipe printed, its last 16 KB. Read to the end whatever it holds: a
/// full pipe would stop the step.
fn read_tail(mut from: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    const KEPT: usize = 16 * 1024;
    std::thread::spawn(move || {
        let mut kept: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8 * 1024];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > 2 * KEPT {
                kept.drain(..kept.len() - KEPT);
            }
        }
        let from = kept.len().saturating_sub(KEPT);
        kept.split_off(from)
    })
}

/// The end of `text`, at most `max` bytes, from a character boundary.
pub(crate) fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_only_its_last_four_kilobytes() {
        let long = format!("{}end", "é".repeat(5000));
        let kept = tail(&long, 4096);
        assert!(kept.len() <= 4096 && kept.ends_with("end"), "{}", kept.len());
        assert_eq!(tail("short", 4096), "short");
    }

    #[test]
    fn the_real_runner_runs_an_argv_in_a_folder_and_hands_back_what_it_printed() {
        let dir = std::env::temp_dir().join(format!("vl-run-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let out = Real.run(&argv(&["pwd"]), &dir, Duration::from_secs(20)).unwrap();
        assert_eq!(Path::new(out.trim()).canonicalize().unwrap(), dir.canonicalize().unwrap());

        // An argument is never read by a shell.
        let out = Real.run(&argv(&["echo", "$HOME; touch pwned"]), &dir, Duration::from_secs(20)).unwrap();
        assert_eq!(out.trim(), "$HOME; touch pwned");
        assert!(!dir.join("pwned").exists());

        let err = Real.run(&argv(&["sh", "-c", "echo boom >&2; exit 3"]), &dir, Duration::from_secs(20)).unwrap_err();
        assert!(err.to_string().contains("boom") && err.to_string().contains('3'), "{err}");

        let err = Real.run(&argv(&["no-such-program-here"]), &dir, Duration::from_secs(20)).unwrap_err();
        assert!(err.to_string().contains("no-such-program-here"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_real_runner_stops_a_step_that_outlives_its_timeout_and_what_it_started() {
        let dir = std::env::temp_dir().join(format!("vl-run-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let started = std::time::Instant::now();

        let argv = ["sh", "-c", "sleep 30 & sleep 30"].map(String::from);
        let err = Real.run(&argv, &dir, Duration::from_millis(300)).unwrap_err();

        assert!(err.to_string().contains("did not finish"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        std::fs::remove_dir_all(&dir).ok();
    }
}
