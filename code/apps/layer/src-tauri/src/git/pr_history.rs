//! Exact departed branch tips from a single worktree's HEAD reflog.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::{command, guessed_default_branch, run, tracked_default_branch};

const MAX_ENTRIES: usize = 100;
const MAX_TOPICS: usize = 3;
const MAX_BYTES: u64 = 256 * 1024;
const MAX_START_BYTES: u64 = 8 * 1024;

#[derive(Debug)]
pub(crate) struct DepartedTopic {
    pub branch: String,
    pub shas: Vec<String>,
}

/// An enclosing repo can answer normally if a nested worktree loses `.git`.
pub(crate) fn verify_checkout_root(dir: &Path) -> Result<()> {
    let git_root = run(dir, &["rev-parse", "--show-toplevel"])?;
    if dir.canonicalize()? != Path::new(git_root.trim_end_matches('\n')).canonicalize()? {
        return Err(Error::Git("The checkout path is not the root of its Git worktree.".into()));
    }
    Ok(())
}

/// Read this checkout's log after verifying its root. A departure's old SHA
/// needs corroboration: Git can rewrite it when it removes intervening entries.
pub(crate) fn departed_topics(
    dir: &Path,
    since: i64,
    base: &str,
    configured_default: Option<&str>,
    excluded: &[&str],
) -> (Vec<DepartedTopic>, Option<Error>) {
    let mut file = match reflog(dir, "HEAD") {
        Ok(file) => file,
        Err(err) => return (Vec::new(), Some(err)),
    };
    let (bytes, truncated) = match tail(&mut file).map_err(|e| log_error("HEAD", e)) {
        Ok(data) => data,
        Err(err) => return (Vec::new(), Some(err)),
    };
    let mut error = None;
    let records = complete_tail(&bytes, truncated);
    let mut lines = records.rsplit(|b| *b == b'\n');
    if records.is_empty() {
        error = Some(Error::Git("The checkout's HEAD reflog is empty or unreadable.".into()));
    } else if lines.next() != Some(b"") {
        error = Some(malformed());
    }
    if truncated && lines.clone().count() < MAX_ENTRIES {
        error = Some(Error::Git("The checkout's HEAD reflog exceeds the history read limit.".into()));
    }

    let records: Vec<_> =
        lines.take(MAX_ENTRIES).map(|line| std::str::from_utf8(line).ok().and_then(parse_record)).collect();
    let mut previous_new = None;
    let mut active = None;
    let mut local_commit_tip = None;
    let mut rename_start: Option<Record<'_>> = None;
    let mut candidates = Vec::new();
    let mut updates = Vec::new();
    for (index, record) in records.into_iter().rev().enumerate() {
        let Some(record) = record else {
            error.get_or_insert_with(malformed);
            previous_new = None;
            active = None;
            local_commit_tip = None;
            rename_start = None;
            continue;
        };
        let gap = previous_new.is_some_and(|new| new != record.old);
        if gap && record.stamp >= since {
            error.get_or_insert_with(|| Error::Git("The checkout's HEAD reflog has a gap.".into()));
        }
        if let Some((from, to)) = checkout_transition(record.action) {
            if record.stamp >= since
                && topic_name(from)
                && !excluded.contains(&from)
                && from != base
                && Some(from) != configured_default
            {
                candidates.push(Departure {
                    branch: from,
                    record,
                    index,
                    known_branch: active == Some(from),
                    previous_new,
                    committed_here: !gap && local_commit_tip == Some(record.old),
                });
            }
            active = topic_name(to).then_some(to);
            local_commit_tip = None;
            rename_start = None;
        } else if let Some((from, to)) = branch_rename(record.action) {
            // Git writes deletion and creation records for a checked-out
            // rename. The same nonzero tip must cross both records.
            let starts = !gap && active == Some(from) && !zero(record.old) && zero(record.new);
            let finishes = !gap
                && zero(record.old)
                && rename_start.is_some_and(|first| first.action == record.action && first.old == record.new);
            let single = !gap && active == Some(from) && record.old == record.new && !zero(record.new);
            active = (finishes || single).then_some(to);
            rename_start = starts.then_some(record);
            if !starts && active.is_none() {
                local_commit_tip = None;
            }
            if active.is_some() {
                updates.push(BranchUpdate { branch: to, index, record: Record { old: record.new, ..record } });
            }
        } else if let Some(branch) = record.action.strip_prefix("rebase (finish): returning to refs/heads/") {
            active = topic_name(branch).then_some(branch);
            local_commit_tip = None;
            rename_start = None;
        } else if gap || record.action.starts_with("rebase (start)") || record.action.starts_with("Branch: renamed ") {
            active = None;
            local_commit_tip = None;
            rename_start = None;
        } else {
            rename_start = None;
            if let Some(branch) = active.filter(|_| record.old != record.new) {
                updates.push(BranchUpdate { branch, index, record });
                local_commit_tip = (record.stamp >= since
                    && (record.action.starts_with("commit: ") || record.action.starts_with("commit (")))
                .then_some(record.new);
            }
        }
        previous_new = Some(record.new);
    }

    if candidates.is_empty() {
        return (Vec::new(), error);
    }
    // origin/HEAD can be absent or stale. Exclude it and the known configured
    // default, but don't guess that main/master is also a default.
    let default =
        tracked_default_branch(dir).or_else(|| configured_default.is_none().then(|| guessed_default_branch(dir)));
    candidates.retain(|candidate| Some(candidate.branch) != default.as_deref());
    if candidates.is_empty() {
        return (Vec::new(), error);
    }
    let Some(point) = checkout_start(&mut file) else {
        let unknown = Error::Git("The checkout's original branch point could not be verified.".into());
        return (Vec::new(), error.or(Some(unknown)));
    };
    let mut found: Vec<DepartedTopic> = Vec::new();
    let mut named_logs = HashMap::new();
    for candidate in candidates.into_iter().rev() {
        let (branch, old) = (candidate.branch, candidate.record.old);
        let at = found.iter().position(|e| e.branch == branch);
        if (at.is_none() && found.len() == MAX_TOPICS) || at.is_some_and(|i| found[i].shas.iter().any(|sha| sha == old))
        {
            continue;
        }
        if !candidate.known_branch {
            error.get_or_insert_with(|| Error::Git(format!("Could not verify the departure from {branch}.")));
            continue;
        }
        // Even an apparent no-work switch can be a rewritten old SHA. Check
        // the named reflog before deciding that the tip equals the start.
        let named = named_logs.entry(branch).or_insert_with(|| NamedEvidence::read(dir, branch));
        let confirmed = match named {
            NamedEvidence::Present(log) => log.confirms(&candidate, &updates),
            // Branch cleanup removes the independent log. Require a real
            // on-branch commit at this tip, not just arrival at that SHA
            // or numerical continuity after a rewritten prune.
            NamedEvidence::Absent => candidate.committed_here,
            NamedEvidence::Unreadable => false,
        };
        if !confirmed {
            error.get_or_insert_with(|| Error::Git(format!("Could not verify the departure tip of {branch}.")));
            continue;
        }
        if old == point {
            continue;
        }
        let range = format!("{point}..{old}");
        match run(dir, &["rev-list", "--max-count=1", &range, "--"]) {
            Ok(out) if !out.trim().is_empty() => {
                if let Some(i) = at {
                    found[i].shas.push(old.into());
                } else {
                    found.push(DepartedTopic { branch: branch.into(), shas: vec![old.into()] });
                }
            }
            Ok(_) => {}
            Err(_) => {
                error.get_or_insert_with(|| Error::Git(format!("Could not verify the recorded tip of {branch}.")));
            }
        }
    }
    (found, error)
}

fn reflog(dir: &Path, name: &str) -> Result<File> {
    File::open(reflog_path(dir, name)?).map_err(|e| log_error(name, e))
}

fn reflog_path(dir: &Path, name: &str) -> Result<PathBuf> {
    // `git reflog show` prints only the NEW object ID; it cannot prove the
    // old tip of a checkout when adjacent entries have been expired or lost.
    let path = run(dir, &["rev-parse", "--path-format=absolute", "--git-path", &format!("logs/{name}")])?;
    Ok(PathBuf::from(path.trim_end_matches('\n')))
}

fn log_error(name: &str, error: std::io::Error) -> Error {
    Error::Git(format!("Could not read the checkout's {name} reflog: {error}."))
}

fn tail(file: &mut File) -> std::io::Result<(Vec<u8>, bool)> {
    let size = file.seek(SeekFrom::End(0))?;
    let start = size.saturating_sub(MAX_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (size - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok((bytes, start != 0))
}

fn complete_tail(bytes: &[u8], truncated: bool) -> &[u8] {
    if truncated {
        bytes.splitn(2, |b| *b == b'\n').nth(1).unwrap_or_default()
    } else {
        bytes
    }
}

fn checkout_start(file: &mut File) -> Option<String> {
    // Read only the creation record outside the recent departure window;
    // neither time spent in the checkout nor base_commit can move this point.
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    BufReader::new(file.take(MAX_START_BYTES)).read_until(b'\n', &mut bytes).ok()?;
    let line = std::str::from_utf8(bytes.strip_suffix(b"\n")?).ok()?;
    let record = parse_record(line)?;
    (record.action.is_empty() && zero(record.old) && !zero(record.new)).then(|| record.new.to_string())
}

struct Departure<'a> {
    branch: &'a str,
    record: Record<'a>,
    index: usize,
    known_branch: bool,
    previous_new: Option<&'a str>,
    committed_here: bool,
}

struct BranchUpdate<'a> {
    branch: &'a str,
    index: usize,
    record: Record<'a>,
}

struct NamedLog {
    tip: String,
    bytes: Vec<u8>,
    truncated: bool,
}

enum NamedEvidence {
    Present(NamedLog),
    Absent,
    Unreadable,
}

impl NamedEvidence {
    fn read(dir: &Path, branch: &str) -> Self {
        let name = format!("refs/heads/{branch}");
        let Ok(path) = reflog_path(dir, &name) else { return Self::Unreadable };
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e)
                if e.kind() == ErrorKind::NotFound
                    && command(dir, &["show-ref", "--verify", "--quiet", "--", &name])
                        .output()
                        .is_ok_and(|out| out.status.code() == Some(1)) =>
            {
                return Self::Absent;
            }
            Err(_) => return Self::Unreadable,
        };
        let Ok((bytes, truncated)) = tail(&mut file) else { return Self::Unreadable };
        let Ok(tip) = run(dir, &["rev-parse", "--verify", "--end-of-options", &format!("{name}^{{commit}}")]) else {
            return Self::Unreadable;
        };
        Self::Present(NamedLog { tip: tip.trim().into(), bytes, truncated })
    }
}

impl NamedLog {
    fn confirms(&self, departure: &Departure<'_>, updates: &[BranchUpdate<'_>]) -> bool {
        let bytes = complete_tail(&self.bytes, self.truncated);
        let Some(bytes) = bytes.strip_suffix(b"\n") else { return false };
        let mut expected = self.tip.as_str();
        let mut upper_stamp = i64::MAX;
        for line in bytes.rsplit(|b| *b == b'\n').take(MAX_ENTRIES) {
            let Some(entry) = std::str::from_utf8(line).ok().and_then(parse_record) else { return false };
            // A missing update or non-monotonic clock makes an as-of read
            // ambiguous. Check the branch ref too, not just its log tail.
            if entry.new != expected || entry.stamp > upper_stamp {
                return false;
            }
            expected = entry.old;
            upper_stamp = entry.stamp;
            if entry.stamp > departure.record.stamp {
                continue;
            }
            if entry.stamp == departure.record.stamp {
                // A later worktree visit can update the branch in the same
                // second. Skip it only if the independent transition lines
                // up with an update known to follow the departure.
                let mut relevant = updates.iter().filter(|u| {
                    u.branch == departure.branch
                        && u.record.stamp == entry.stamp
                        && u.record.old == entry.old
                        && u.record.new == entry.new
                });
                let later = relevant.clone().any(|u| u.index > departure.index);
                let earlier = relevant.any(|u| u.index < departure.index);
                if later {
                    if earlier {
                        return false;
                    }
                    continue;
                }
                // Otherwise an unplaced same-second update can have happened
                // after we left. It proves a tip only if it created the branch
                // or fills a genuine gap between the observed HEAD and the
                // claimed tip. A reset back to an arrival SHA does neither.
                let fills_gap = departure.previous_new == Some(entry.old) && entry.old != departure.record.old;
                if !earlier && !zero(entry.old) && !fills_gap {
                    return false;
                }
            }
            return entry.new == departure.record.old;
        }
        false
    }
}

fn malformed() -> Error {
    Error::Git("The checkout's HEAD reflog contains a malformed entry.".into())
}

#[derive(Clone, Copy)]
struct Record<'a> {
    old: &'a str,
    new: &'a str,
    stamp: i64,
    action: &'a str,
}

fn parse_record(line: &str) -> Option<Record<'_>> {
    // Worktree creation writes a valid first entry without an action.
    let (header, action) = line.split_once('\t').unwrap_or((line, ""));
    let (old, header) = header.split_once(' ')?;
    let (new, identity) = header.split_once(' ')?;
    let (_, date) = identity.rsplit_once("> ")?;
    let (stamp, zone) = date.split_once(' ')?;
    if old.len() != new.len()
        || !is_oid(old)
        || !is_oid(new)
        || zone.len() != 5
        || !matches!(zone.as_bytes()[0], b'+' | b'-')
        || !zone.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(Record { old, new, stamp: stamp.parse().ok()?, action })
}

fn checkout_transition(action: &str) -> Option<(&str, &str)> {
    action.strip_prefix("checkout: moving from ")?.split_once(" to ").filter(|(_, to)| !to.is_empty())
}

fn branch_rename(action: &str) -> Option<(&str, &str)> {
    let (from, to) = action.strip_prefix("Branch: renamed refs/heads/")?.split_once(" to refs/heads/")?;
    (topic_name(from) && topic_name(to)).then_some((from, to))
}

fn topic_name(name: &str) -> bool {
    // A detached checkout names the old object instead of a branch.
    !name.is_empty()
        && name != "HEAD"
        && !is_oid(name)
        && !name.starts_with('-')
        && !name.starts_with("refs/")
        && !name.contains("@{")
        && !name.contains("..")
        && !name.bytes().any(|b| b <= b' ' || b == 127 || b"~^:?*[\\".contains(&b))
        && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
        && !name.ends_with('.')
}

fn is_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn zero(value: &str) -> bool {
    value.bytes().all(|b| b == b'0')
}
