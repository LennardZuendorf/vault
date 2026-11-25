//! Committing some files and no others: the spec Commit spec puts (SPEC-35)
//! in a worktree where an agent may be halfway through its own change.

use std::path::Path;

use std::io::Write;
use std::process::Stdio;

use super::{command, in_progress, run};
use crate::error::{Error, Result};

/// Commit what is under `paths` (relative to `dir`), added, changed or
/// removed, and nothing else: whatever else is staged stays staged, and is
/// not in the commit. None when there was nothing to commit there.
/// `message` is a sentence, written as the repository writes its own
/// (`in_house_style`).
pub fn commit_only(dir: &Path, paths: &[&str], message: &str) -> Result<Option<String>> {
    if paths.is_empty() {
        return Ok(None);
    }
    if let Some(busy) = in_progress(dir) {
        return Err(Error::Git(format!(
            "a {} is in progress here — resolve it or abandon it before committing",
            busy.word()
        )));
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend_from_slice(paths);
    run(dir, &add)?;
    // Nothing of these paths changed: committing would fail on it, or with
    // `--allow-empty` say something that is not so.
    let mut status = vec!["status", "--porcelain", "--untracked-files=normal", "--"];
    status.extend_from_slice(paths);
    if run(dir, &status)?.trim().is_empty() {
        return Ok(None);
    }
    // `--only` takes these paths as they are and leaves the rest of the
    // index alone, so an agent's staged work is neither committed nor
    // unstaged.
    let message = in_house_style(dir, message);
    let mut commit = vec!["commit", "--only", "-m", &message, "--"];
    commit.extend_from_slice(paths);
    run(dir, &commit)?;
    Ok(Some(run(dir, &["rev-parse", "HEAD"])?.trim().to_string()))
}

/// `sentence` as a commit message the repository at `dir` takes. One that
/// writes conventional commits, or lints them, gets `docs: tick the …`: its
/// commitlint hook refused "Tick the done steps of the spec for ACME-1837."
/// for having no type and a full stop, and Open pull requests stopped there.
fn in_house_style(dir: &Path, sentence: &str) -> String {
    if !writes_conventional_commits(dir) {
        return sentence.to_string();
    }
    let subject = sentence.trim().trim_end_matches('.');
    let mut chars = subject.chars();
    let first = chars.next().map(|c| c.to_lowercase().collect::<String>()).unwrap_or_default();
    format!("docs: {first}{}", chars.as_str())
}

/// Whether the repository has a commitlint config, or most of its recent
/// commits are `type(scope): subject`. The config is what a commit-msg hook
/// reads; the history is for a repo that checks only in CI. Most, not all:
/// squash merges keep titles like "Acme 1479 tests".
fn writes_conventional_commits(dir: &Path) -> bool {
    const CONFIGS: [&str; 15] = [
        "commitlint.config.js",
        "commitlint.config.cjs",
        "commitlint.config.mjs",
        "commitlint.config.ts",
        "commitlint.config.cts",
        "commitlint.config.mts",
        ".commitlintrc",
        ".commitlintrc.json",
        ".commitlintrc.yaml",
        ".commitlintrc.yml",
        ".commitlintrc.js",
        ".commitlintrc.cjs",
        ".commitlintrc.mjs",
        ".commitlintrc.ts",
        ".commitlintrc.cts",
    ];
    if CONFIGS.iter().any(|f| dir.join(f).is_file())
        || std::fs::read_to_string(dir.join("package.json")).is_ok_and(|p| p.contains("\"commitlint\""))
    {
        return true;
    }
    let Ok(log) = run(dir, &["log", "-20", "--format=%s"]) else { return false };
    let subjects: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    let conventional = subjects.iter().filter(|s| is_conventional(s)).count();
    !subjects.is_empty() && conventional * 2 > subjects.len()
}

/// `feat: …`, `fix(ACME-1): …`, `chore(deps)!: …`.
fn is_conventional(subject: &str) -> bool {
    let Some((head, rest)) = subject.split_once(": ") else { return false };
    let head = head.strip_suffix('!').unwrap_or(head);
    let kind = match head.split_once('(') {
        Some((kind, scope)) => match scope.strip_suffix(')') {
            Some(scope) if !scope.is_empty() => kind,
            _ => return false,
        },
        None => head,
    };
    !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase()) && !rest.trim().is_empty()
}

/// The commit `dir` is on.
pub fn head_commit(dir: &Path) -> Result<String> {
    Ok(run(dir, &["rev-parse", "HEAD"])?.trim().to_string())
}

/// What each of `rels` held at the commit `dir` is on, in one git process
/// (Commit spec asks this of every spec file, every time the Pull requests
/// tab looks). None for one it held nothing at (not in it, or no commit
/// yet), or nothing that is text. Commit spec compares this with what it
/// would write, so a spec file committed by hand is not "changed".
pub fn committed_texts(dir: &Path, rels: &[&str]) -> Result<Vec<Option<String>>> {
    // A name with a line break in it cannot be asked for on a line.
    let asked: Vec<&str> = rels.iter().copied().filter(|r| !r.contains('\n')).collect();
    if asked.is_empty() {
        return Ok(vec![None; rels.len()]);
    }
    let mut child = command(dir, &["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Git(format!("failed to run git in {}: {e}", dir.display())))?;
    let input: String = asked.iter().map(|r| format!("HEAD:{r}\n")).collect();
    let mut stdin = child.stdin.take().ok_or_else(|| Error::Git("git took no input".into()))?;
    // Written beside the read: a big answer would fill the pipe and stop git
    // halfway through the questions.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output().map_err(|e| Error::Git(format!("git cat-file failed: {e}")))?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(Error::Git(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let mut answers = parse_batch(&out.stdout).into_iter();
    Ok(rels.iter().map(|r| if r.contains('\n') { None } else { answers.next().flatten() }).collect())
}

/// `git cat-file --batch`'s answers, in order: a blob's text, or None.
fn parse_batch(mut out: &[u8]) -> Vec<Option<String>> {
    let mut answers = Vec::new();
    while let Some(end) = out.iter().position(|&b| b == b'\n') {
        let head = String::from_utf8_lossy(&out[..end]).to_string();
        out = &out[end + 1..];
        let parts: Vec<&str> = head.split(' ').collect();
        match parts[..] {
            [_, kind, size] => {
                let size: usize = size.parse().unwrap_or(0).min(out.len());
                let body = &out[..size];
                out = out.get(size + 1..).unwrap_or_default();
                answers.push((kind == "blob").then(|| String::from_utf8(body.to_vec()).ok()).flatten());
            }
            // `<name> missing`, `<name> ambiguous`
            _ => answers.push(None),
        }
    }
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vl-only-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]).unwrap();
        run(&dir, &["config", "user.email", "test@villain.local"]).unwrap();
        run(&dir, &["config", "user.name", "Test"]).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&dir, &["add", "-A"]).unwrap();
        run(&dir, &["commit", "-qm", "init"]).unwrap();
        dir
    }

    #[test]
    fn only_the_given_files_are_committed_and_an_agents_staged_work_stays_staged() {
        let dir = repo();
        // An agent's work: one change staged, one not.
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        run(&dir, &["add", "a.txt"]).unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        std::fs::create_dir_all(dir.join("specs/T-1")).unwrap();
        std::fs::write(dir.join("specs/T-1/requirements.md"), "## Goal\n").unwrap();

        let sha = commit_only(&dir, &["specs/T-1"], "Approve the requirements for T-1.").unwrap();
        assert!(sha.is_some());
        let files = run(&dir, &["show", "--name-only", "--pretty=format:", "HEAD"]).unwrap();
        assert_eq!(files.trim(), "specs/T-1/requirements.md");
        let status = run(&dir, &["status", "--porcelain"]).unwrap();
        assert!(status.contains("M  a.txt"), "the staged change is still staged: {status}");
        assert!(status.contains("?? b.txt"), "{status}");

        // Nothing new: no commit.
        assert_eq!(commit_only(&dir, &["specs/T-1"], "again").unwrap(), None);

        // Removed: the removal is committed.
        std::fs::remove_file(dir.join("specs/T-1/requirements.md")).unwrap();
        assert!(commit_only(&dir, &["specs/T-1"], "Remove it.").unwrap().is_some());
        let files = run(&dir, &["show", "--name-status", "--pretty=format:", "HEAD"]).unwrap();
        assert_eq!(files.trim(), "D\tspecs/T-1/requirements.md");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repository_that_writes_conventional_commits_gets_one() {
        let dir = repo();
        for subject in
            ["feat(ACME-1): favourite stations", "chore(deps)!: bump vitest", "fix: the date", "Acme 1479 tests"]
        {
            std::fs::write(dir.join("a.txt"), subject).unwrap();
            run(&dir, &["commit", "-qam", subject]).unwrap();
        }
        std::fs::create_dir_all(dir.join("specs/T-1")).unwrap();
        std::fs::write(dir.join("specs/T-1/tasks.md"), "- [x] 1. Do it\n").unwrap();

        commit_only(&dir, &["specs/T-1"], "Tick the done steps of the spec for T-1.").unwrap();
        let subject = run(&dir, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(subject.trim(), "docs: tick the done steps of the spec for T-1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_commitlint_config_asks_for_a_conventional_commit_whatever_the_history_says() {
        let dir = repo();
        std::fs::write(dir.join("commitlint.config.js"), "module.exports = {};\n").unwrap();
        assert_eq!(in_house_style(&dir, "Approve the design for T-1."), "docs: approve the design for T-1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn any_other_repository_gets_the_sentence() {
        let dir = repo();
        assert_eq!(in_house_style(&dir, "Approve the design for T-1."), "Approve the design for T-1.");
        assert!(!is_conventional("Keep new files: when a task is finished."));
        assert!(!is_conventional("feat(): nothing in the scope"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn committed_texts_answers_every_name_in_one_go_and_none_for_what_head_lacks() {
        let dir = repo();
        std::fs::create_dir_all(dir.join("d")).unwrap();
        std::fs::write(dir.join("d/b.md"), "two\nlines\n").unwrap();
        run(&dir, &["add", "-A"]).unwrap();
        run(&dir, &["commit", "-qm", "b"]).unwrap();
        std::fs::write(dir.join("a.txt"), "changed, not committed\n").unwrap();

        let got = committed_texts(&dir, &["a.txt", "nope.md", "d/b.md", "d", "bad\nname"]).unwrap();

        assert_eq!(got, [Some("one\n".to_string()), None, Some("two\nlines\n".to_string()), None, None]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
