//! Kiro's extras (§19): drafting a spec's files, applying the answers to its
//! open questions, and checking the work against it. Each a fresh one-shot
//! run, never the agent doing the work, and each only for a toolset that lists
//! the extra in its manifest.
//!
//! They read and write the working copy's files like anything else that
//! touches the spec: a draft is written into the file (`write_spec`) and what
//! it replaced is returned, so the tab can offer Undo. Nothing is held back
//! as a draft elsewhere.
//!
//! Every file is drafted in one run for the whole task, however many
//! repositories it touches (SPEC-5): the requirements, so the split of the
//! work is decided in one place; the design, reading every repository's
//! code, so both sides of an endpoint are described once; and every
//! repository's tasks, so what comes first across them is decided in one
//! place. A run per repository wrote each side of a contract in its own
//! words, and nothing compared them.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::config::Task;
use crate::error::{Error, Result};
use crate::oneshot::{oneshot, Oneshot};
use crate::shellenv;
use crate::spec::owned::own_part;
use crate::spec::toolset::{Extra, Toolset};
use crate::spec::{self, Kind, Part};

use super::jira::{review_context, reviewed_repos, task_repos};
use super::panes::agent_file_dir;
use super::spec::{keep_check, Check, Verdict};
use super::spec_commit::{repositories, Repo};
use super::spec_copy::{read_file, spec_dir, toolset_of};
use super::spec_prompts::{answers_prompt, check_prompt, design_prompt, requirements_prompt, tasks_prompt};
use super::spec_view::CheckView;
use super::spec_work::{update_spec, with_spec_moved, write_spec};
use super::task_context::TICKET_FILE;
use super::AppState;

/// A piece of a draft under way, as the tab's editor shows it: the whole text
/// of the file so far.
#[derive(Clone, Serialize)]
struct SpecDraftChunk<'a> {
    request_id: &'a str,
    /// The file being drafted, relative to the spec folder.
    path: &'a str,
    text: &'a str,
}

/// What a draft did: the file it wrote and what that file held before (None
/// when there was none), for Undo.
#[derive(Debug, Serialize)]
pub struct Drafted {
    pub path: String,
    pub before: Option<String>,
}

/// What Apply answers did to the requirements (SPEC-20), by id, and what the
/// file held before, for Undo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppliedAnswers {
    pub path: String,
    pub before: String,
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// How a run is made: its prompt, where it stands, whether it may read the
/// files there, and a hand for the text as it streams in. The app's is
/// `claude -p`; tests give the model's answer.
type RunFn<'a> = dyn FnMut(&str, &Path, bool, &mut dyn FnMut(&str)) -> Result<String> + 'a;
type Run<'a> = &'a mut RunFn<'a>;

/// Sonnet without thinking: what a ticket means is a judgement Haiku makes
/// thinly, and thinking would leave the user watching an empty editor.
fn how(read: bool) -> Oneshot<'static> {
    Oneshot { model: "sonnet", read }
}

fn claude() -> Result<String> {
    shellenv::which("claude").ok_or_else(|| Error::NotFound("claude is not on your PATH".into()))
}

/// The run the app makes.
fn real(program: String) -> Box<RunFn<'static>> {
    Box::new(move |prompt, dir, read, on_delta| oneshot(&program, dir, how(read), prompt, on_delta))
}

fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// The toolset of `task`, when it has `extra`: these are Kiro's, and another
/// toolset has its own way of drafting. Checked before anything is run.
fn require(state: &AppState, task: &Task, extra: Extra) -> Result<Toolset> {
    let t = toolset_of(state, task);
    if t.extras.contains(&extra) {
        return Ok(t);
    }
    let what = match extra {
        Extra::Draft => "Draft",
        Extra::Check => "Check against spec",
        Extra::Questions => "Apply answers",
    };
    Err(Error::Other(format!("{} has no {what}: it is an extra of Kiro's toolset", t.name)))
}

/// A file of the spec, or None when it is not there yet.
fn text_of(dir: &Path, t: &Toolset, rel: &str) -> Result<Option<String>> {
    match read_file(dir, t, rel) {
        Ok(text) => Ok(Some(text)),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Feature or bug: a spec with a `bugfix.md` is a bug's, else a feature's. A
/// requirements file that is there decides it, so a draft never makes a second.
fn kind_of(dir: &Path, t: &Toolset, asked: Option<Kind>) -> Result<Kind> {
    for kind in [Kind::Bugfix, Kind::Feature] {
        if text_of(dir, t, Part::Requirements.file(kind))?.is_some() {
            return Ok(kind);
        }
    }
    Ok(asked.unwrap_or_default())
}

/// The requirements as they are written now, and their file.
fn requirements_of(dir: &Path, t: &Toolset) -> Result<Option<(&'static str, String)>> {
    let rel = Part::Requirements.file(kind_of(dir, t, None)?);
    Ok(text_of(dir, t, rel)?.map(|text| (rel, text)))
}

/// Where a run that reads code stands: the task folder, which holds every
/// repository's worktree, or the one worktree of a task without one.
fn reading_dir(state: &AppState, task: &Task, repos: &[Repo]) -> PathBuf {
    agent_file_dir(state, task)
        .or_else(|| repos.first().map(|r| PathBuf::from(&r.checkout.path)))
        .unwrap_or_else(std::env::temp_dir)
}

/// Draft one file of the spec (SPEC-5), streamed as `spec:draft` into its
/// editor and then written into the file: the requirements (a bug's analysis,
/// when the spec is a bug's or `kind` says so for the first draft), the design,
/// or every repository's tasks. Returns what the file held before.
#[tauri::command]
pub async fn draft_spec(
    app: AppHandle,
    task_id: String,
    part: Part,
    kind: Option<Kind>,
    request_id: String,
) -> Result<Drafted> {
    {
        let state = app.state::<AppState>();
        require(&state, &state.config.task(&task_id)?, Extra::Draft)?;
    }
    let program = claude()?;
    let emitter = app.clone();
    // An epic's tasks go to the tickets already under it first (EPIC-4).
    // Without them a draft planned their work again, and filing it made a
    // second ticket for each.
    let under: Vec<(String, String)> = match part {
        Part::Tasks => {
            let task = app.state::<AppState>().config.task(&task_id)?;
            match (task.epic, task.issue_key.as_deref()) {
                (true, Some(key)) => super::epics::tickets_under(&app, &task)
                    .await
                    .map_err(|e| Error::Other(format!("the tickets under {key} could not be read: {e}")))?
                    .issues
                    .into_iter()
                    .map(|t| (t.key, t.summary))
                    .collect(),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };
    super::blocking(app, move |state| {
        let say = |path: &str, text: &str| {
            let _ = emitter.emit("spec:draft", SpecDraftChunk { request_id: &request_id, path, text });
        };
        draft_inner(state, &state.config.task(&task_id)?, part, kind, &under, &say, &mut *real(program))
    })
    .await
}

fn draft_inner(
    state: &AppState,
    task: &Task,
    part: Part,
    kind: Option<Kind>,
    under: &[(String, String)],
    say: &dyn Fn(&str, &str),
    run: Run,
) -> Result<Drafted> {
    let t = require(state, task, Extra::Draft)?;
    let task = &with_spec_moved(state, task)?;
    let dir = spec_dir(state, task);
    let repos = repositories(state, task);
    if repos.is_empty() {
        return Err(Error::Other("this task has no repositories to write a spec for".into()));
    }
    let folders: Vec<String> = repos.iter().map(|r| r.folder.clone()).collect();
    let kind = kind_of(&dir, &t, kind)?;
    let wanted = || Error::Other("write the requirements first".into());
    // What goes into the file: a whole text, or the run's parts for the tasks,
    // which are put together with what the file holds when it is written.
    let (rel, body) = match part {
        Part::Requirements => {
            let rel = Part::Requirements.file(kind);
            let ticket = agent_file_dir(state, task).and_then(|d| std::fs::read_to_string(d.join(TICKET_FILE)).ok());
            // What the user answered on the draft before is decided: a
            // redraft from the ticket alone asked it all again and threw the
            // answers away (SPEC-5).
            let decided = text_of(&dir, &t, rel)?.map(|text| spec::answered(&text)).unwrap_or_default();
            let prompt =
                requirements_prompt(task, kind, &folders, &task_repos(state, task), ticket.as_deref(), &decided);
            let mut so_far = String::new();
            let out = run(&prompt, &home_dir(), false, &mut |delta| {
                so_far.push_str(delta);
                say(rel, &so_far);
            })?;
            if out.trim().is_empty() {
                return Err(Error::Other("claude returned no requirements".into()));
            }
            (rel, Body::Whole(out))
        }
        Part::Design => {
            let (_, reqs) = requirements_of(&dir, &t)?.ok_or_else(wanted)?;
            let old = text_of(&dir, &t, "design.md")?;
            let prompt = design_prompt(kind, &folders, &reqs, old.as_deref());
            let mut so_far = String::new();
            let out = run(&prompt, &reading_dir(state, task, &repos), true, &mut |delta| {
                so_far.push_str(delta);
                say("design.md", &so_far);
            })?;
            if out.trim().is_empty() {
                return Err(Error::Other("claude returned an empty design".into()));
            }
            ("design.md", Body::Whole(out))
        }
        Part::Tasks => {
            let (_, reqs) = requirements_of(&dir, &t)?.ok_or_else(wanted)?;
            let design = text_of(&dir, &t, "design.md")?.unwrap_or_else(|| "None.".into());
            let old = own_sections(&text_of(&dir, &t, "tasks.md")?.unwrap_or_default(), &folders);
            let prompt = tasks_prompt(&folders, &reqs, &design, &old, task.epic.then_some(under));
            let mut so_far = String::new();
            let out = run(&prompt, &home_dir(), false, &mut |delta| {
                so_far.push_str(delta);
                say("tasks.md", &sectioned(&folders, &by_repo(&so_far, &folders), &[]));
            })?;
            let split = by_repo(&out, &folders);
            if split.iter().all(|(_, t)| spec::steps(t).is_empty()) {
                return Err(Error::Other("claude returned no steps".into()));
            }
            ("tasks.md", Body::Tasks(split, spec::requirements(&reqs)))
        }
    };
    // Written from the file as it is now: the run took minutes, and an agent
    // may have ticked a step in them. Done stays done, and a step left without
    // its requirements is marked, not dropped (SPEC-7).
    let (before, out) = update_spec(state, task, rel, |current| {
        Ok(match body {
            Body::Whole(text) => text,
            Body::Tasks(split, requirements) => {
                let old = own_sections(current.unwrap_or_default(), &folders);
                let kept: Vec<(String, String)> = split
                    .into_iter()
                    .map(|(folder, text)| {
                        let before = old.iter().find(|(f, _)| *f == folder).map(|(_, own)| own.as_str());
                        let mine: Vec<_> = requirements.iter().filter(|r| r.names(&folder)).cloned().collect();
                        let text = match before {
                            Some(before) => spec::sync_steps(before, &text, &mine),
                            None => text,
                        };
                        (folder, text)
                    })
                    .collect();
                sectioned(&folders, &kept, &old)
            }
        })
    })?;
    say(rel, &out);
    Ok(Drafted { path: rel.to_string(), before })
}

/// What a draft is to write: a whole text, or the tasks' parts by repository
/// with the requirements they serve.
enum Body {
    Whole(String),
    Tasks(Vec<(String, String)>, Vec<spec::Requirement>),
}

/// Each repository's steps of a `tasks.md`, without the heading that opens its
/// section; a repository with none is left out.
fn own_sections(file: &str, folders: &[String]) -> Vec<(String, String)> {
    folders
        .iter()
        .map(|f| (f.clone(), without_heading(&own_part(file, f, folders), f)))
        .filter(|(_, own)| !spec::steps(own).is_empty())
        .collect()
}

/// `text` without the `# <folder>` line that opens a repository's section.
fn without_heading(text: &str, folder: &str) -> String {
    let text = text.trim_start();
    match text.split_once('\n') {
        Some((first, rest))
            if first.trim().strip_prefix("# ").is_some_and(|h| h.trim().eq_ignore_ascii_case(folder)) =>
        {
            rest.trim_start().to_string()
        }
        _ => text.to_string(),
    }
}

/// The tasks of every repository as one `tasks.md`: with one repository, its
/// text; with more, a first-level `# <folder>` section each, which is how a
/// step file says whose steps are whose (a second-level heading, like the
/// `## Tasks` inside each, would end the section). A repository the run left
/// out keeps what it had (`before`).
fn sectioned(folders: &[String], drafted: &[(String, String)], before: &[(String, String)]) -> String {
    let text_of =
        |folder: &String| drafted.iter().chain(before).find(|(f, _)| f == folder).map(|(_, t)| t.trim().to_string());
    if let [only] = folders {
        return format!("{}\n", text_of(only).unwrap_or_default());
    }
    let parts: Vec<String> = folders
        .iter()
        .filter_map(|f| text_of(f).filter(|t| !t.is_empty()).map(|t| format!("# {f}\n\n{t}\n")))
        .collect();
    parts.join("\n")
}

/// Build the answered open questions into the requirements (SPEC-20): one
/// run over the requirements as the file holds them (the tab saves them
/// first). Streamed as `spec:draft`, then written into the file; what it held
/// before is returned, for Undo.
#[tauri::command]
pub async fn apply_spec_answers(app: AppHandle, task_id: String, request_id: String) -> Result<AppliedAnswers> {
    {
        let state = app.state::<AppState>();
        require(&state, &state.config.task(&task_id)?, Extra::Questions)?;
    }
    let program = claude()?;
    let emitter = app.clone();
    super::blocking(app, move |state| {
        let say = |path: &str, text: &str| {
            let _ = emitter.emit("spec:draft", SpecDraftChunk { request_id: &request_id, path, text });
        };
        apply_inner(state, &state.config.task(&task_id)?, &say, &mut *real(program))
    })
    .await
}

fn apply_inner(state: &AppState, task: &Task, say: &dyn Fn(&str, &str), run: Run) -> Result<AppliedAnswers> {
    let t = require(state, task, Extra::Questions)?;
    let task = &with_spec_moved(state, task)?;
    let dir = spec_dir(state, task);
    let (rel, before) =
        requirements_of(&dir, &t)?.ok_or_else(|| Error::Other("write the requirements first".into()))?;
    if spec::answered(&before).is_empty() {
        return Err(Error::Other("no open question has an answer yet".into()));
    }
    let folders: Vec<String> = repositories(state, task).into_iter().map(|r| r.folder).collect();
    let mut so_far = String::new();
    let out = run(&answers_prompt(&folders, &before), &home_dir(), false, &mut |delta| {
        so_far.push_str(delta);
        say(rel, &so_far);
    })?;
    if out.trim().is_empty() {
        say(rel, &before);
        return Err(Error::Other("claude returned nothing".into()));
    }
    say(rel, &out);
    write_spec(state, task, rel, &out)?;
    let (changed, added, removed) = compare(&before, &out);
    Ok(AppliedAnswers { path: rel.to_string(), before, changed, added, removed })
}

/// Check one repository's work against the requirements that name it
/// (SPEC-15): a run that may read the worktree, over the requirements and the
/// design as the working copy holds them now, and the branch's changes from
/// its branch point. Kept with the commit it was checked at.
#[tauri::command]
pub async fn check_spec(app: AppHandle, task_id: String, checkout_id: String) -> Result<CheckView> {
    {
        let state = app.state::<AppState>();
        require(&state, &state.config.task(&task_id)?, Extra::Check)?;
    }
    let program = claude()?;
    super::blocking(app, move |state| {
        check_inner(state, &state.config.task(&task_id)?, &checkout_id, &mut *real(program))
    })
    .await
}

fn check_inner(state: &AppState, task: &Task, checkout_id: &str, run: Run) -> Result<CheckView> {
    let t = require(state, task, Extra::Check)?;
    let task = &with_spec_moved(state, task)?;
    let dir = spec_dir(state, task);
    let repo = repositories(state, task)
        .into_iter()
        .find(|r| r.checkout.id == checkout_id)
        .ok_or_else(|| Error::NotFound(format!("checkout {checkout_id}")))?;
    let (_, requirements) =
        requirements_of(&dir, &t)?.ok_or_else(|| Error::Other("there are no requirements to check against".into()))?;
    let ids: Vec<String> =
        spec::requirements(&requirements).into_iter().filter(|r| r.names(&repo.folder)).map(|r| r.id).collect();
    if ids.is_empty() {
        return Err(Error::Other(format!("no requirement names {}", repo.folder)));
    }
    let design = text_of(&dir, &t, "design.md")?;
    let reviewed: Vec<_> = reviewed_repos(state, task).into_iter().filter(|r| r.checkout_id == checkout_id).collect();
    let sha = crate::git::head_commit(Path::new(&repo.checkout.path))?;
    let prompt = check_prompt(&repo.folder, &ids, &requirements, design.as_deref(), &review_context(&reviewed));
    let out = run(&prompt, Path::new(&repo.checkout.path), true, &mut |_| {})?;
    let results = verdicts(&out, &ids);
    if results.is_empty() {
        return Err(Error::Other("the check gave no answer for any requirement".into()));
    }
    let check = Check { sha, at: chrono::Utc::now().timestamp_millis(), results };
    keep_check(state, task, &repo.folder, &check)?;
    Ok(CheckView { check, stale: false })
}

/// A run's text, one part per name: a first-level heading that is a name
/// alone starts its part. With one name, no heading is needed.
fn by_repo(text: &str, names: &[String]) -> Vec<(String, String)> {
    let named = |line: &str| {
        let name = line.strip_prefix("# ")?.trim().trim_matches('`').trim_end_matches('/');
        names.iter().find(|f| f.eq_ignore_ascii_case(name)).cloned()
    };
    let mut parts: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Some(folder) = named(line.trim_end()) {
            parts.push((folder, String::new()));
        } else if let Some((_, part)) = parts.last_mut() {
            part.push_str(line);
        } else if names.len() == 1 {
            parts.push((names[0].clone(), line.to_string()));
        }
    }
    parts.into_iter().map(|(f, t)| (f, t.trim_start().to_string())).collect()
}

/// The requirements changed, added and removed between two texts, by id.
fn compare(before: &str, after: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let old = spec::requirements(before);
    let new = spec::requirements(after);
    let normal = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let changed = new
        .iter()
        .filter(|n| old.iter().any(|o| o.id == n.id && (normal(&o.text) != normal(&n.text) || o.repos != n.repos)))
        .map(|n| n.id.clone())
        .collect();
    let added = new.iter().filter(|n| !old.iter().any(|o| o.id == n.id)).map(|n| n.id.clone()).collect();
    let removed = old.iter().filter(|o| !new.iter().any(|n| n.id == o.id)).map(|o| o.id.clone()).collect();
    (changed, added, removed)
}

/// The check's lines, one per requirement it knows: `R-1 | met | why`.
fn verdicts(out: &str, ids: &[String]) -> Vec<Verdict> {
    let mut found: Vec<Verdict> = Vec::new();
    for line in out.lines() {
        let mut cells = line.trim().trim_matches('|').splitn(3, '|').map(str::trim);
        let (Some(id), Some(verdict)) = (cells.next(), cells.next()) else { continue };
        let id = id.trim_matches('*').to_ascii_uppercase();
        let verdict = verdict.trim_matches('*').trim_matches('`').to_ascii_lowercase();
        if !ids.contains(&id) || found.iter().any(|v| v.id == id) {
            continue;
        }
        let verdict = match verdict.as_str() {
            "met" => "met",
            "not met" | "unmet" => "not met",
            _ => "unclear",
        };
        found.push(Verdict { id, verdict: verdict.into(), evidence: cells.next().unwrap_or("").to_string() });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task {
            toolset: None,
            id: "t1".into(),
            name: "ACME-12 Refunds\nfail for split payments".into(),
            root: "/work/acme-12".into(),
            branch: "ACME-12".into(),
            issue_key: Some("ACME-12".into()),
            issue_url: None,
            created_at: chrono::Utc::now(),
            ticket_stage: None,
            chat: None,
            review: None,
            browser: None,
            browser_url: None,
            epic: false,
            sub_task: None,
        }
    }

    fn two() -> Vec<String> {
        vec!["api".to_string(), "web".to_string()]
    }

    #[test]
    fn a_run_for_two_repositories_is_split_at_their_headings() {
        let text = "# api\n## Tasks\n- [ ] 1. Add it\n\n# `web`\n## Tasks\n- [ ] 1. Show it\n";
        let parts = by_repo(text, &two());
        assert_eq!(
            parts,
            vec![
                ("api".into(), "## Tasks\n- [ ] 1. Add it\n\n".into()),
                ("web".into(), "## Tasks\n- [ ] 1. Show it\n".into())
            ]
        );
        // One repository needs no heading, and is not cut without one.
        assert_eq!(by_repo("## Tasks\nx\n", &two()[..1]), vec![("api".into(), "## Tasks\nx\n".into())]);
        // Half way through, what has come so far.
        assert_eq!(by_repo("# api\n## Ta", &two()), vec![("api".into(), "## Ta".into())]);
    }

    #[test]
    fn applying_answers_names_what_changed_by_id() {
        let before = "## Requirements\n- R-1: WHEN a sixth reset THE SYSTEM SHALL refuse it.\n- R-2 [api]: WHEN two addresses THE SYSTEM SHALL count apart.\n- R-3: WHEN staff ask THE SYSTEM SHALL lift it.\n";
        let after = "## Requirements\n- R-1: WHEN a sixth reset THE SYSTEM SHALL refuse it, saying when to try again.\n- R-2 [api, web]: WHEN  two addresses THE SYSTEM SHALL count apart.\n- R-4: WHEN a reset is refused THE SYSTEM SHALL log it.\n";
        let (changed, added, removed) = compare(before, after);
        assert_eq!(changed, vec!["R-1", "R-2"], "spacing alone is no change; another repository is");
        assert_eq!(added, vec!["R-4"]);
        assert_eq!(removed, vec!["R-3"]);
    }

    /// Whether the model names repositories and builds answers in. Run with
    /// `cargo test --lib real_ -- --ignored`.
    #[test]
    #[ignore = "runs claude for real"]
    fn real_requirements_for_two_repositories_name_the_repositories_that_meet_them() {
        let program = shellenv::which("claude").expect("claude on the PATH");
        let ticket = "## The ticket: ACME-12 Rate-limit sign-in\n\n> After five failed sign-ins in a minute, \
                      the API refuses more for that address, and the sign-in page says when to try again.";
        let prompt = requirements_prompt(&task(), Kind::Feature, &two(), &[], Some(ticket), &[]);
        let out = oneshot(&program, std::env::temp_dir(), how(false), &prompt, |_| {}).unwrap();
        let reqs = spec::requirements(&out);
        assert!(!reqs.is_empty(), "{out}");
        assert!(reqs.iter().any(|r| r.names("api") && !r.repos.is_empty()), "{out}");
        assert!(reqs.iter().any(|r| r.names("web") && !r.repos.is_empty()), "{out}");
        eprintln!("{out}");
    }

    #[test]
    #[ignore = "runs claude for real"]
    fn real_answers_are_built_in_and_only_answered_questions_go() {
        let program = shellenv::which("claude").expect("claude on the PATH");
        let before = "## Goal\nPassword resets are rate-limited.\n\n## Requirements\n\
                      - R-1: WHEN a sixth reset is asked for one address within an hour THE SYSTEM SHALL refuse it.\n\
                      - R-2: WHEN resets are asked for different addresses THE SYSTEM SHALL count them separately.\n\n\
                      ## Out of scope\n- Rate-limiting sign-ins.\n\n## Open questions\n\
                      - Q-1: Should the refusal say when to try again?\n  - Yes, the minutes left\n  - No\n  Answer: Yes, the minutes left\n\
                      - Q-2: Should support staff be able to lift the limit?\n  - Yes\n  - No\n-- no, not in this work\n\
                      - Q-3: Is the hour a sliding window or a clock hour?\n  - Sliding\n  - Clock hour\n";
        let out =
            oneshot(&program, std::env::temp_dir(), how(false), &answers_prompt(&two()[..1], before), |_| {}).unwrap();
        let (changed, added, removed) = compare(before, &out);
        assert!(
            changed.contains(&"R-1".to_string()) || !added.is_empty(),
            "the refusal's wording is in no requirement:\n{out}"
        );
        assert!(
            !changed.contains(&"R-2".to_string()) && removed.is_empty(),
            "an untouched requirement changed:\n{out}"
        );
        assert!(spec::answered(&out).is_empty(), "an answered question is still there:\n{out}");
        assert!(out.contains("sliding window or a clock hour"), "the unanswered question went:\n{out}");
        assert!(!out.contains("Should support staff"), "{out}");
        eprintln!("{out}");
    }

    #[test]
    fn a_checks_answers_are_read_for_known_requirements_only() {
        let out = "Here you go:\nR-1 | met | api/src/limit.ts:12\n| R-2 | **Not met** | no 429 |\nR-9 | met | ?\nR-3 | maybe | can't tell\n";
        let v = verdicts(out, &["R-1".into(), "R-2".into(), "R-3".into()]);
        assert_eq!(v.len(), 3);
        assert_eq!(
            (v[0].id.as_str(), v[0].verdict.as_str(), v[0].evidence.as_str()),
            ("R-1", "met", "api/src/limit.ts:12")
        );
        assert_eq!(v[1].verdict, "not met");
        assert_eq!(v[2].verdict, "unclear");
    }

    use crate::commands::spec_world::{get, put, world};

    fn spec_at(root: &Path, rel: &str) -> PathBuf {
        root.join("tasks/T-1/spec").join(rel)
    }

    fn quiet(_: &str, _: &str) {}

    /// A run that answers `text` whatever it is asked, and keeps the prompt.
    fn answering<'a>(text: &'a str, asked: &'a std::cell::RefCell<Vec<String>>) -> Box<RunFn<'a>> {
        Box::new(move |prompt, _, _, on_delta| {
            asked.borrow_mut().push(prompt.to_string());
            on_delta(text);
            Ok(text.to_string())
        })
    }

    fn refusing(_: &str, _: &Path, _: bool, _: &mut dyn FnMut(&str)) -> Result<String> {
        Err(Error::Other("a toolset without the extra must not run anything".into()))
    }

    #[test]
    fn drafting_is_refused_outside_kiro() {
        let (root, state, task) = world(&["api"]);
        state.config.update(|c| c.tasks.iter_mut().for_each(|t| t.toolset = Some("openspec".into()))).unwrap();
        let task = state.config.task(&task.id).unwrap();

        let said = |e: Error| e.to_string();
        let draft = draft_inner(&state, &task, Part::Requirements, None, &[], &quiet, &mut refusing).unwrap_err();
        assert_eq!(said(draft), "OpenSpec has no Draft: it is an extra of Kiro's toolset");
        let apply = apply_inner(&state, &task, &quiet, &mut refusing).unwrap_err();
        assert!(said(apply).contains("has no Apply answers"));
        let check = check_inner(&state, &task, "c-api", &mut refusing).unwrap_err();
        assert!(said(check).contains("has no Check against spec"));
        assert!(!spec_at(&root, "requirements.md").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_kiro_draft_writes_the_file_and_returns_what_it_replaced() {
        let (root, state, task) = world(&["api"]);
        let asked = std::cell::RefCell::new(Vec::new());

        let first = draft_inner(
            &state,
            &task,
            Part::Requirements,
            None,
            &[],
            &quiet,
            &mut *answering("## Requirements\n- R-1: WHEN a THE SYSTEM SHALL b.\n", &asked),
        )
        .unwrap();
        assert_eq!((first.path.as_str(), first.before), ("requirements.md", None));
        assert!(get(&spec_at(&root, "requirements.md")).contains("R-1"));

        let again = draft_inner(
            &state,
            &task,
            Part::Requirements,
            None,
            &[],
            &quiet,
            &mut *answering("## Requirements\n- R-1: new\n", &asked),
        )
        .unwrap();
        assert_eq!(again.before.as_deref(), Some("## Requirements\n- R-1: WHEN a THE SYSTEM SHALL b.\n"));
        assert_eq!(get(&spec_at(&root, "requirements.md")), "## Requirements\n- R-1: new\n");
        assert!(!spec_at(&root, ".spec-drafts").exists());

        // A bug is drafted into the file a bug's requirements are in, once.
        let (root2, state2, task2) = world(&["api"]);
        let bug = draft_inner(
            &state2,
            &task2,
            Part::Requirements,
            Some(Kind::Bugfix),
            &[],
            &quiet,
            &mut *answering("## Expected behaviour\n- R-1: x\n", &asked),
        )
        .unwrap();
        assert_eq!(bug.path, "bugfix.md");
        let redo = draft_inner(
            &state2,
            &task2,
            Part::Requirements,
            Some(Kind::Feature),
            &[],
            &quiet,
            &mut *answering("## Expected behaviour\n- R-1: y\n", &asked),
        )
        .unwrap();
        assert_eq!(redo.path, "bugfix.md", "the file that is there decides");
        assert!(!spec_at(&root2, "requirements.md").exists());
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&root2).ok();
    }

    #[test]
    fn the_design_and_the_tasks_need_the_requirements_and_tasks_keep_what_was_done() {
        let (root, state, task) = world(&["api", "web"]);
        let asked = std::cell::RefCell::new(Vec::new());
        let err =
            draft_inner(&state, &task, Part::Design, None, &[], &quiet, &mut *answering("d", &asked)).unwrap_err();
        assert_eq!(err.to_string(), "write the requirements first");

        put(&spec_at(&root, "requirements.md"), "## Requirements\n- R-1 [api, web]: WHEN a THE SYSTEM SHALL b.\n");
        let design =
            draft_inner(&state, &task, Part::Design, None, &[], &quiet, &mut *answering("## Approach\nx\n", &asked))
                .unwrap();
        assert_eq!(design.path, "design.md");
        assert!(asked.borrow().last().unwrap().contains("WHEN a THE SYSTEM SHALL b."), "it reads the working copy");

        put(
            &spec_at(&root, "tasks.md"),
            "# api\n\n## Tasks\n- [x] 1. Add it (R-1)\n\n# web\n\n## Tasks\n- [ ] 1. Show it (R-1)\n",
        );
        let run = "# api\n## Tasks\n- [ ] 1. Add it (R-1)\n- [ ] 2. Test it (R-1)\n\n# web\n## Tasks\n- [ ] 1. Show it (R-1; after api 1)\n";
        let tasks = draft_inner(&state, &task, Part::Tasks, None, &[], &quiet, &mut *answering(run, &asked)).unwrap();

        assert_eq!(tasks.path, "tasks.md");
        assert!(tasks.before.unwrap().contains("[x] 1. Add it"));
        let written = get(&spec_at(&root, "tasks.md"));
        assert!(written.contains("# api\n\n## Tasks\n- [x] 1. Add it"), "done stays done: {written}");
        assert!(written.contains("# web\n\n## Tasks\n- [ ] 1. Show it (R-1; after api 1)"), "{written}");
        let folders = vec!["api".to_string(), "web".to_string()];
        let steps = crate::spec::owned::owned_steps(&written, &folders);
        assert_eq!(steps.iter().filter(|s| s.repos == ["api"]).count(), 2, "{steps:?}");
        assert_eq!(steps.iter().filter(|s| s.repos == ["web"]).count(), 1, "{steps:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_step_ticked_while_the_tasks_are_being_drafted_stays_ticked() {
        let (root, state, task) = world(&["api"]);
        put(&spec_at(&root, "requirements.md"), "## Requirements\n- R-1: WHEN a THE SYSTEM SHALL b.\n");
        put(&spec_at(&root, "tasks.md"), "## Tasks\n- [ ] 1. Add it (R-1)\n- [ ] 2. Test it (R-1)\n");
        // The run takes minutes; an agent ticks step 1 meanwhile.
        let mut run = |_: &str, _: &Path, _: bool, _: &mut dyn FnMut(&str)| -> Result<String> {
            crate::commands::spec_work::tick_step(&state, "t1", None, 1, true).unwrap();
            Ok("## Tasks\n- [ ] 1. Add it (R-1)\n- [ ] 2. Test it (R-1)\n- [ ] 3. Log it (R-1)\n".into())
        };

        let drafted = draft_inner(&state, &task, Part::Tasks, None, &[], &quiet, &mut run).unwrap();

        let written = get(&spec_at(&root, "tasks.md"));
        assert!(written.contains("- [x] 1. Add it"), "the tick survives: {written}");
        assert!(written.contains("3. Log it"), "{written}");
        assert!(drafted.before.unwrap().contains("[x] 1. Add it"), "Undo restores what was there, tick and all");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn applying_answers_rewrites_the_requirements_file_and_hands_back_the_old_text() {
        let (root, state, task) = world(&["api"]);
        let asked = std::cell::RefCell::new(Vec::new());
        let before = "## Requirements\n- R-1: WHEN a THE SYSTEM SHALL b.\n\n## Open questions\n- Q-1: Say when?\n  Answer: yes\n";
        put(&spec_at(&root, "requirements.md"), before);

        let done = apply_inner(
            &state,
            &task,
            &quiet,
            &mut *answering("## Requirements\n- R-1: WHEN a THE SYSTEM SHALL b, saying when.\n", &asked),
        )
        .unwrap();

        assert_eq!(
            (done.path.as_str(), done.before.as_str(), done.changed.as_slice()),
            ("requirements.md", before, ["R-1".to_string()].as_slice())
        );
        assert!(get(&spec_at(&root, "requirements.md")).contains("saying when"));
        put(&spec_at(&root, "requirements.md"), "## Requirements\n- R-1: x\n");
        let none = apply_inner(&state, &task, &quiet, &mut refusing).unwrap_err();
        assert_eq!(none.to_string(), "no open question has an answer yet");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn check_against_spec_reads_the_working_copy() {
        let (root, state, task) = world(&["api", "web"]);
        let api = root.join("tasks/T-1/api");
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@villain.local"],
            &["config", "user.name", "T"],
            &["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            crate::git::run_for_tests(&api, args).unwrap();
        }
        put(
            &spec_at(&root, "requirements.md"),
            "## Requirements\n- R-1 [api]: WHEN a THE SYSTEM SHALL b.\n- R-2 [web]: WHEN c THE SYSTEM SHALL d.\n",
        );
        put(&spec_at(&root, "design.md"), "## Contracts\n- C-1: POST /refunds\n");
        let asked = std::cell::RefCell::new(Vec::new());

        let checked = check_inner(
            &state,
            &task,
            "c-api",
            &mut *answering("R-1 | met | api/x.rs:1\nR-2 | met | not ours", &asked),
        )
        .unwrap();

        let prompt = asked.borrow()[0].clone();
        assert!(
            prompt.contains("R-1") && prompt.contains("WHEN a THE SYSTEM SHALL b.") && prompt.contains("POST /refunds"),
            "{prompt}"
        );
        assert!(prompt.contains("requirements it must meet: R-1."), "{prompt}");
        assert_eq!(checked.check.results.len(), 1, "only the requirements that name api");
        assert!(!checked.stale);
        let kept = crate::commands::spec::check_of(&state, &task, "api").unwrap();
        assert_eq!(kept.sha, checked.check.sha);
        assert!(get(&root.join("tasks/T-1/.spec-checks/api.json")).contains("R-1"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sections_keep_a_repository_the_run_left_out() {
        let folders = vec!["api".to_string(), "web".to_string()];
        let drafted = vec![("api".to_string(), "## Tasks\n- [ ] 1. a".to_string())];
        let before = vec![("web".to_string(), "## Tasks\n- [x] 1. w".to_string())];
        assert_eq!(
            sectioned(&folders, &drafted, &before),
            "# api\n\n## Tasks\n- [ ] 1. a\n\n# web\n\n## Tasks\n- [x] 1. w\n"
        );
        assert_eq!(sectioned(&folders[..1], &drafted, &[]), "## Tasks\n- [ ] 1. a\n");
        assert_eq!(without_heading("# api\n\n## Tasks\n- [ ] 1. a\n", "api"), "## Tasks\n- [ ] 1. a\n");
    }
}
