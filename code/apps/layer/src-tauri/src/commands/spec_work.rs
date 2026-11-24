//! Working to a task's spec (§19): what agents are told of it, the steps
//! they tick, what the pull requests say of it, and telling them it changed.

use std::path::Path;

use serde_json::{json, Value};
use tauri::State;

use crate::config::{SpecCommit, Task};
use crate::error::{Error, Result};
use crate::git;
use crate::mcp::{arg, required};
use crate::spec::owned::{owned_steps, tick_at, OwnedStep};
use crate::spec::toolset::Toolset;
use crate::spec::{self, glob, Kind, Part};

use super::spec::{and_list, spec_of, Slot};
use super::spec_commit::commit_spec;
use super::spec_copy::{list_files, move_old_spec, read_file, spec_dir, toolset_of, write_file};
use super::AppState;

/// Every edit of a spec file that reads the file and writes it back, a tick
/// or a whole new text: one at a time. Two agents ticking at once each wrote
/// back what they had read, and one tick was lost.
static EDITING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// The task a spec tool is about: its `task_id`, else the calling pane's.
pub(crate) fn named_task(state: &AppState, args: &Value, caller: Option<&str>) -> Result<Task> {
    let id = match arg(args, "task_id") {
        Some(id) => id.to_string(),
        None => caller
            .and_then(|pane| state.ptys.info(pane).ok())
            .map(|p| p.task_id)
            .ok_or_else(|| Error::Other("give task_id".into()))?,
    };
    state.config.task(&id)
}

/// `task`, with a spec kept the old way moved into its `spec/` folder first
/// when there is none yet. The move may pin the task to Kiro, so the task is
/// read again after it.
fn with_spec_moved(state: &AppState, task: &Task) -> Result<Task> {
    if std::fs::symlink_metadata(spec_dir(state, task)).is_err() && move_old_spec(state, task)? {
        return state.config.task(&task.id);
    }
    Ok(task.clone())
}

/// The task's repositories as (folder, project name): what a step file and
/// `spec_commit` call them.
fn repositories(state: &AppState, task: &Task) -> Vec<(String, String)> {
    state
        .config
        .checkouts_of(&task.id)
        .into_iter()
        .filter_map(|c| {
            let folder = Path::new(&c.path).file_name()?.to_string_lossy().to_string();
            let name = state.config.project(&c.project_id).ok()?.name;
            Some((folder, name))
        })
        .collect()
}

fn folders_of(state: &AppState, task: &Task) -> Vec<String> {
    repositories(state, task).into_iter().map(|(folder, _)| folder).collect()
}

fn holds_steps(t: &Toolset, rel: &str) -> bool {
    t.steps.iter().any(|g| glob::matches(g, rel))
}

/// The step file a tick means: `file`, or the only one the spec has.
fn step_file(dir: &Path, t: &Toolset, file: Option<&str>) -> Result<String> {
    let found: Vec<String> = list_files(dir, t)?.into_iter().filter(|f| holds_steps(t, f)).collect();
    match (file, found.as_slice()) {
        (_, []) => Err(Error::Other("the spec has no file of steps yet".into())),
        (Some(f), _) if found.iter().any(|s| s == f) => Ok(f.to_string()),
        (Some(f), _) => {
            Err(Error::Other(format!("{f} is not a file of steps in this spec; they are: {}", found.join(", "))))
        }
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(Error::Other(format!("give file: this spec has more than one, {}", found.join(", ")))),
    }
}

/// Tick (or untick) the step at `position` of a step file, for an agent's
/// `spec_task` (SPEC-13): `file` when the spec has more than one. The step as
/// it is now.
pub(crate) fn tick_step(
    state: &AppState,
    task_id: &str,
    file: Option<&str>,
    position: u32,
    done: bool,
) -> Result<OwnedStep> {
    let task = with_spec_moved(state, &state.config.task(task_id)?)?;
    let (t, dir) = (toolset_of(state, &task), spec_dir(state, &task));
    let _one = EDITING.lock();
    let rel = step_file(&dir, &t, file)?;
    let text = read_file(&dir, &t, &rel)?;
    let ticked = tick_at(&text, position, done).ok_or_else(|| Error::NotFound(format!("step {position} of {rel}")))?;
    write_file(&dir, &t, &rel, &ticked)?;
    owned_steps(&ticked, &folders_of(state, &task))
        .into_iter()
        .find(|s| s.position == position)
        .ok_or_else(|| Error::NotFound(format!("step {position} of {rel}")))
}

/// What a spec tool answers, and the task whose spec it changed, so the
/// caller can say so.
#[derive(Debug)]
pub struct SpecAnswer {
    pub value: Value,
    pub changed: Option<String>,
}

/// One of `spec_read`, `spec_write`, `spec_task` and `spec_commit`: the
/// task's spec as files, for an agent that has no other way to reach the task
/// folder.
pub fn spec_tool(state: &AppState, name: &str, args: &Value, caller: Option<&str>) -> Result<SpecAnswer> {
    let named = named_task(state, args, caller)?;
    let task = with_spec_moved(state, &named)?;
    let (t, dir) = (toolset_of(state, &task), spec_dir(state, &task));
    let unchanged = |value| SpecAnswer { value, changed: None };
    match name {
        "spec_read" => match arg(args, "path") {
            Some(path) => Ok(unchanged(Value::String(read_file(&dir, &t, path)?))),
            None => {
                let mut rows = Vec::new();
                for path in list_files(&dir, &t)? {
                    let steps = if holds_steps(&t, &path) {
                        owned_steps(&read_file(&dir, &t, &path)?, &[])
                    } else {
                        Vec::new()
                    };
                    rows.push(json!({
                        "path": path,
                        "steps_done": steps.iter().filter(|s| s.done).count(),
                        "steps_total": steps.len(),
                    }));
                }
                Ok(unchanged(Value::Array(rows)))
            }
        },
        "spec_write" => {
            let path = required(args, "path")?;
            // An empty text is a file made empty, so only a missing one is refused.
            let text =
                args.get("text").and_then(Value::as_str).ok_or_else(|| Error::Other("text is required".into()))?;
            {
                let _one = EDITING.lock();
                write_file(&dir, &t, path, text)?;
            }
            Ok(SpecAnswer { value: Value::String(format!("wrote {path}")), changed: Some(task.id) })
        }
        "spec_task" => {
            let step = args.get("step").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
            let step = step.ok_or_else(|| Error::Other("step is required: its position in the file, from 1".into()))?;
            let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
            let ticked = tick_step(state, &task.id, arg(args, "file"), step, done)?;
            Ok(SpecAnswer { value: serde_json::to_value(ticked)?, changed: Some(task.id) })
        }
        "spec_commit" => {
            super::pr_task::not_a_review(&task, "given a spec of its own")?;
            let known = repositories(state, &task);
            let only = match args.get("repos").and_then(Value::as_array) {
                Some(names) => Some(
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|n| {
                            known
                                .iter()
                                .find(|(folder, project)| folder == n || project == n)
                                .map(|(folder, _)| folder.clone())
                                .ok_or_else(|| Error::NotFound(format!("a repository `{n}` in this task")))
                        })
                        .collect::<Result<Vec<_>>>()?,
                ),
                None => None,
            };
            Ok(unchanged(serde_json::to_value(commit_spec(state, &task, only.as_deref()))?))
        }
        other => Err(Error::NotFound(format!("tool {other}"))),
    }
}

/// Commit what agents ticked since the last approval, before a push takes
/// the branch to review (SPEC-13): `tasks.md` alone, and only while it
/// differs from what was approved in its ticks. A spec changed outside the
/// app is the user's to approve or put back (SPEC-22), not to be committed
/// as ticks.
///
/// Open pull requests commits the whole spec now (`spec_commit.rs`); this goes
/// with the old layout.
#[allow(dead_code)]
pub(crate) fn commit_ticks(state: &AppState, task: &Task, checkout_id: &str) -> Result<()> {
    let spec = spec_of(state, task);
    let Some(home) = spec.homes.iter().find(|h| h.checkout.id == checkout_id) else { return Ok(()) };
    let Some(rel) = &home.rel else { return Ok(()) };
    let file = Part::Tasks.file(Kind::Feature);
    if !home.dir.join(file).is_file() || spec.has_outside(&Slot::tasks(&home.folder)) {
        return Ok(());
    }
    let message = format!("Tick the done steps of the spec for {}.", task.issue_key.as_deref().unwrap_or(&task.branch));
    git::commit_only(Path::new(&home.checkout.path), &[format!("{rel}/{file}").as_str()], &message)?;
    Ok(())
}

/// The sentence that tells an agent to write the spec first (Spec first,
/// SPEC-27), naming the working copy by its full path.
pub(crate) fn spec_first_line(dir: &Path) -> String {
    format!("Write or update the spec in {} before you change any code.", dir.display())
}

/// That sentence, when the setting is on and the task is one an agent writes
/// code in: a review changes none, and has no spec of its own.
pub(crate) fn spec_first_for(state: &AppState, task: &Task) -> Option<String> {
    (state.config.with(|c| c.spec_first) && task.review.is_none()).then(|| spec_first_line(&spec_dir(state, task)))
}

/// What an opening prompt gains for a task's spec (SPEC-11): where the
/// working copy is, once it holds files, and with Spec first on the line
/// that tells the agent to write it before any code, which it gets even
/// while there is nothing to point at yet. Reads the disk.
pub(crate) fn prompt_line(state: &AppState, task: &Task) -> Option<String> {
    if task.review.is_some() {
        return None;
    }
    let (t, dir) = (toolset_of(state, task), spec_dir(state, task));
    let written = list_files(&dir, &t).is_ok_and(|files| !files.is_empty());
    let pointer = written.then(|| {
        format!(
            "The spec for this work is in `{}`, one for the whole task, whatever repository you work in. \
             It is what done means; read it before you start.",
            dir.display()
        )
    });
    let parts: Vec<String> = pointer.into_iter().chain(spec_first_for(state, task)).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// `prompt` with `prompt_line` after it: the one way an opening prompt gets
/// the spec's part, so none leaves out the Spec first line.
pub(crate) fn with_spec_line(state: &AppState, task: &Task, prompt: String) -> String {
    match prompt_line(state, task) {
        Some(line) => format!("{prompt}\n\n{line}"),
        None => prompt,
    }
}

/// The spec's part of a drafted pull request description (SPEC-16): for
/// each repository, where its spec is in it and the requirements it meets,
/// read from the working copy's entry file, with the latest check's answer.
/// Runs git.
pub(crate) fn pr_section(state: &AppState, task: &Task) -> String {
    let (t, dir) = (toolset_of(state, task), spec_dir(state, task));
    let reqs = super::task_context::entry_file(&t, &dir).map(|(_, text)| spec::requirements(&text)).unwrap_or_default();
    let spec = spec_of(state, task);
    let mut md = String::new();
    for h in &spec.homes {
        let mine: Vec<_> = reqs.iter().filter(|r| r.names(&h.folder)).collect();
        if mine.is_empty() {
            continue;
        }
        // Kiro's goes under the repository's spec folder; the other toolsets
        // mirror their own layout (`commit_to`).
        let rel = if t.id == "kiro" { h.rel.clone() } else { t.commit_to.clone() };
        let place = match rel.filter(|_| h.project.spec_commit != SpecCommit::None) {
            Some(rel) => format!("`{rel}/` in the repository"),
            None => "kept outside the repository, so not linked".into(),
        };
        md.push_str(&format!("\n## Spec of `{}`: {place}\n\n", h.project.name));
        let check = spec.check(h);
        let head = git::head_commit(Path::new(&h.checkout.path)).ok();
        let stale = check.as_ref().is_some_and(|c| head.as_deref() != Some(c.sha.as_str()));
        for r in mine {
            let answer = check
                .as_ref()
                .and_then(|c| c.results.iter().find(|v| v.id == r.id))
                .map(|v| {
                    format!(
                        "{}{}",
                        v.verdict,
                        if v.evidence.is_empty() { String::new() } else { format!(" ({})", v.evidence) }
                    )
                })
                .unwrap_or_else(|| "not checked".into());
            md.push_str(&format!("- {}: {} — {answer}\n", r.id, r.text));
        }
        match (&check, stale) {
            (None, _) => md.push_str("\nNo check against the spec was run.\n"),
            (Some(_), true) => md.push_str("\nThe check was run before the latest commits.\n"),
            _ => {}
        }
    }
    md
}

/// What Start agent asks of an agent on the spec (SPEC-12): every step file,
/// or one repository's steps in it, in order, each ticked as soon as it is
/// done.
#[tauri::command]
pub async fn spec_work_prompt(app: tauri::AppHandle, task_id: String, checkout_id: Option<String>) -> Result<String> {
    super::blocking(app, move |state| work_prompt(state, &task_id, checkout_id.as_deref())).await
}

/// The body of `spec_work_prompt`; empty when the spec has no step to do.
pub(crate) fn work_prompt(state: &AppState, task_id: &str, checkout_id: Option<&str>) -> Result<String> {
    let task = with_spec_moved(state, &state.config.task(task_id)?)?;
    let (t, dir) = (toolset_of(state, &task), spec_dir(state, &task));
    let folders = folders_of(state, &task);
    // One repository's own: the folder its checkout has.
    let alone = match checkout_id {
        Some(id) => {
            let checkout = state.config.checkout(id)?;
            let folder = Path::new(&checkout.path).file_name().map(|n| n.to_string_lossy().to_string());
            folder.filter(|_| folders.len() > 1)
        }
        None => None,
    };
    let mut lists = Vec::new();
    for rel in list_files(&dir, &t)?.into_iter().filter(|f| holds_steps(&t, f)) {
        let steps = owned_steps(&read_file(&dir, &t, &rel)?, &folders);
        let mine = steps
            .iter()
            .filter(|s| {
                alone.as_ref().is_none_or(|f| s.repos.is_empty() || s.repos.iter().any(|r| r.eq_ignore_ascii_case(f)))
            })
            .count();
        if mine > 0 {
            let of = alone.as_ref().map(|f| format!(" for `{f}`")).unwrap_or_default();
            lists.push(format!("- {} ({mine} step{}{of})", dir.join(&rel).display(), if mine == 1 { "" } else { "s" }));
        }
    }
    if lists.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "Work through the steps in\n{}\nin order. A step marked `after api 2` waits for step 2 of the `api` \
         section: do that one first{}. As soon as a step is done, before you start the next, tick it: call \
         `spec_task` on the `villain-layer` MCP server with the `file` and the step's position in it (counted \
         from 1 over every `- [ ]` box), or edit its box to `[x]` in the file yourself. The user follows the \
         work by these ticks. Do not save them up. If a step turns out wrong or the spec does not hold, change \
         the spec (`spec_write`, or edit its files), say what you changed, and go on from there.",
        lists.join("\n"),
        if alone.is_some() {
            ", and if it is another repository's that is not ticked yet, stop and say which step you are waiting for"
        } else {
            ""
        },
    ))
}

/// Tell the task's running agents that the spec changed (SPEC-14): one line
/// each, naming the files. Returns how many were told.
#[tauri::command]
pub async fn tell_spec_change(app: tauri::AppHandle, task_id: String, parts: Vec<Part>) -> Result<usize> {
    super::blocking(app, move |state| {
        let task = state.config.task(&task_id)?;
        let spec = spec_of(state, &task);
        let kind = spec.kind();
        let mut files: Vec<String> = Vec::new();
        for part in &parts {
            match spec.homes.first() {
                Some(first) if *part != Part::Tasks || spec.homes.len() == 1 => {
                    files.push(format!("{} (the same in every repository)", first.dir.join(part.file(kind)).display()));
                }
                _ => files.extend(spec.homes.iter().map(|h| h.dir.join(part.file(kind)).display().to_string())),
            }
        }
        let line = format!(
            "The user changed the spec for this work: {}. Read it again, and check what you are doing still meets it.",
            and_list(&files.iter().map(String::as_str).collect::<Vec<_>>())
        );
        let mut told = 0;
        for pane in state.ptys.list(Some(&task.id)) {
            if pane.running && pane.kind == crate::pty::PaneKind::Agent {
                super::hand_over(state, &task, &pane.id, "SPEC_CHANGED.md", &line)?;
                told += 1;
            }
        }
        Ok(told)
    })
    .await
}

/// How a repository takes specs (SPEC-1, SPEC-3): the toolset Start work
/// offers for it, what Commit spec puts in it (`none`: the app keeps the
/// spec), and the folder it goes to.
#[tauri::command]
pub fn set_project_specs(
    state: State<AppState>,
    project_id: String,
    toolset: Option<String>,
    commit: SpecCommit,
    folder: String,
) -> Result<()> {
    let folder = spec::valid_folder(&folder).map_err(Error::Other)?;
    let toolset = toolset.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    // The same rule a manifest's id follows, so a typo is refused here
    // rather than saved as a toolset nothing can match.
    if let Some(id) = toolset.as_deref().filter(|id| !spec::toolset::valid_id(id)) {
        return Err(Error::Other(format!("{id:?} can't be a toolset's id: lowercase letters, digits, - or _")));
    }
    state.config.update(|c| {
        if let Some(p) = c.projects.iter_mut().find(|p| p.id == project_id) {
            p.spec_toolset = toolset;
            p.spec_commit = commit;
            p.spec_folder = folder;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::spec_world::{get, put, world};

    fn steps_file(n: u32) -> String {
        (1..=n).map(|n| format!("- [ ] {n}. Step {n}\n")).collect()
    }

    fn spec_at(root: &Path, rel: &str) -> std::path::PathBuf {
        root.join("tasks/T-1/spec").join(rel)
    }

    fn use_toolset(state: &AppState, id: &str) {
        state.config.update(|c| c.tasks.iter_mut().for_each(|t| t.toolset = Some(id.into()))).unwrap();
    }

    #[test]
    fn concurrent_ticks_of_one_file_all_land() {
        let (root, state, _) = world(&["api"]);
        put(&spec_at(&root, "tasks.md"), &steps_file(16));

        std::thread::scope(|s| {
            for n in 1..=16 {
                let state = &state;
                s.spawn(move || tick_step(state, "t1", None, n, true).unwrap());
            }
        });

        let ticked = owned_steps(&get(&spec_at(&root, "tasks.md")), &[]);
        assert_eq!(ticked.len(), 16);
        assert!(ticked.iter().all(|s| s.done), "{ticked:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_tick_answers_with_the_step_and_an_untick_takes_it_back() {
        let (root, state, _) = world(&["api"]);
        put(&spec_at(&root, "tasks.md"), "## api\n\n- [ ] 1.1 Add the route\n- [ ] 1.2 Test it\n");

        let step = tick_step(&state, "t1", Some("tasks.md"), 2, true).unwrap();
        assert_eq!((step.position, step.label.as_str(), step.done), (2, "1.2", true));
        assert_eq!(step.repos, ["api"], "the step as the file's section owns it");
        assert_eq!(get(&spec_at(&root, "tasks.md")), "## api\n\n- [ ] 1.1 Add the route\n- [x] 1.2 Test it\n");

        assert!(!tick_step(&state, "t1", None, 2, false).unwrap().done);
        assert!(tick_step(&state, "t1", None, 3, true).is_err(), "no such step");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_task_without_a_file_is_refused_when_two_step_files_exist() {
        let (root, state, _) = world(&["api"]);
        use_toolset(&state, "superpowers");
        put(&spec_at(&root, "docs/superpowers/plans/one.md"), &steps_file(2));
        put(&spec_at(&root, "docs/superpowers/plans/two.md"), &steps_file(2));

        let refused = tick_step(&state, "t1", None, 1, true).unwrap_err().to_string();
        assert!(refused.contains("give file") && refused.contains("plans/one.md"), "{refused}");
        assert!(get(&spec_at(&root, "docs/superpowers/plans/one.md")).starts_with("- [ ]"), "nothing was ticked");

        tick_step(&state, "t1", Some("docs/superpowers/plans/two.md"), 1, true).unwrap();
        assert!(get(&spec_at(&root, "docs/superpowers/plans/two.md")).starts_with("- [x]"));
        let other = tick_step(&state, "t1", Some("docs/superpowers/specs/x.md"), 1, true).unwrap_err().to_string();
        assert!(other.contains("not a file of steps"), "{other}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_read_lists_files_with_their_steps_and_reads_one() {
        let (root, state, _) = world(&["api"]);
        put(&spec_at(&root, "requirements.md"), "# R\n");
        put(&spec_at(&root, "tasks.md"), "- [x] 1. a\n- [ ] 2. b\n- [ ] 3. c\n");
        let ask = |args: Value| spec_tool(&state, "spec_read", &args, None).map(|a| a.value);

        assert_eq!(
            ask(json!({ "task_id": "t1" })).unwrap(),
            json!([
                { "path": "requirements.md", "steps_done": 0, "steps_total": 0 },
                { "path": "tasks.md", "steps_done": 1, "steps_total": 3 },
            ])
        );
        assert_eq!(ask(json!({ "task_id": "t1", "path": "requirements.md" })).unwrap(), json!("# R\n"));
        assert!(ask(json!({ "task_id": "t1", "path": "../config.json" })).is_err());
        assert!(ask(json!({ "task_id": "t1", "path": "secrets.txt" })).is_err(), "not a file the toolset keeps");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_write_replaces_a_file_and_says_which_task_changed() {
        let (root, state, _) = world(&["api"]);
        let write = |args: Value| spec_tool(&state, "spec_write", &args, None);

        let done = write(json!({ "task_id": "t1", "path": "design.md", "text": "# D\n" })).unwrap();
        assert_eq!((done.value, done.changed.as_deref()), (json!("wrote design.md"), Some("t1")));
        assert_eq!(get(&spec_at(&root, "design.md")), "# D\n");

        assert!(write(json!({ "task_id": "t1", "path": "../escape.md", "text": "x" })).is_err());
        assert!(write(json!({ "task_id": "t1", "path": "notes.txt", "text": "x" })).is_err());
        assert!(write(json!({ "task_id": "t1", "path": "design.md" })).is_err(), "no text");
        assert_eq!(get(&spec_at(&root, "design.md")), "# D\n", "a refused write leaves the file");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spec_tool_moves_an_old_layout_spec_first() {
        let (root, state, _) = world(&["api"]);
        // Where earlier builds kept it: drafts in the task folder.
        let old = root.join("tasks/T-1/.spec-drafts/approved");
        put(&old.join("requirements.md"), "# Old\n");
        assert!(state.config.task("t1").unwrap().toolset.is_none());

        let listed = spec_tool(&state, "spec_read", &json!({ "task_id": "t1" }), None).unwrap().value;
        assert!(listed.to_string().contains("requirements.md"), "{listed}");
        assert_eq!(get(&spec_at(&root, "requirements.md")), "# Old\n");
        assert_eq!(state.config.task("t1").unwrap().toolset.as_deref(), Some("kiro"), "pinned by the move");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_write_from_a_pane_writes_into_its_tasks_spec_folder() {
        use crate::pty::{PaneKind, SpawnOptions};
        let (root, state, _) = world(&["api"]);
        let app = tauri::test::mock_app();
        let pane = state
            .ptys
            .spawn(
                app.handle(),
                SpawnOptions {
                    task_id: "t1".into(),
                    checkout_id: None,
                    cwd: root.to_string_lossy().to_string(),
                    kind: PaneKind::Agent,
                    title: "claude".into(),
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "sleep 5".into()],
                    agent_id: Some("claude".into()),
                    rows: None,
                    cols: None,
                    initial_input: None,
                    prompted: true,
                    env: Vec::new(),
                    title_activity: None,
                    title_topic: None,
                },
            )
            .unwrap()
            .id;

        let done = spec_tool(&state, "spec_write", &json!({ "path": "requirements.md", "text": "# R\n" }), Some(&pane));

        let _ = state.ptys.close(&pane);
        assert_eq!(done.unwrap().changed.as_deref(), Some("t1"));
        assert_eq!(get(&spec_at(&root, "requirements.md")), "# R\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_commit_names_a_repository_it_does_not_have() {
        let (root, state, _) = world(&["api"]);
        let refused = spec_tool(&state, "spec_commit", &json!({ "task_id": "t1", "repos": ["web"] }), None);
        assert!(refused.unwrap_err().to_string().contains("`web`"));
        std::fs::remove_dir_all(&root).ok();
    }

    fn on(state: &AppState) {
        state.config.update(|c| c.spec_first = true).unwrap();
    }

    const FIRST: &str = "Write or update the spec in ";

    #[test]
    fn spec_first_puts_the_line_at_the_end_of_the_opening_prompt() {
        let (root, state, task) = world(&["api"]);
        on(&state);
        let dir = root.join("tasks/T-1/spec");
        let line = format!("{FIRST}{} before you change any code.", dir.display());

        // Even with no spec written yet, which is when it matters.
        assert_eq!(prompt_line(&state, &task).as_deref(), Some(line.as_str()));
        put(&spec_at(&root, "requirements.md"), "# R\n");
        let both = prompt_line(&state, &task).unwrap();
        assert!(both.contains(&format!("is in `{}`", dir.display())), "{both}");
        assert!(both.ends_with(&line), "{both}");
        assert_eq!(with_spec_line(&state, &task, "Task: x".into()), format!("Task: x\n\n{both}"));

        // What task_prompt builds without a ticket, and then adds the line to.
        let task = state.config.task("t1").unwrap();
        let prompt = with_spec_line(&state, &task, crate::commands::jira::plain_prompt(&task, &[], true));
        assert!(prompt.starts_with("Task: T-1 Refunds"), "{prompt}");
        assert!(prompt.ends_with(&line), "{prompt}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_first_off_adds_nothing_and_a_task_without_a_spec_gets_no_line() {
        let (root, state, task) = world(&["api"]);
        assert_eq!(prompt_line(&state, &task), None);
        assert_eq!(with_spec_line(&state, &task, "Task: x".into()), "Task: x");

        put(&spec_at(&root, "requirements.md"), "# R\n");
        let pointing = prompt_line(&state, &task).unwrap();
        assert!(pointing.contains(&root.join("tasks/T-1/spec").display().to_string()), "{pointing}");
        assert!(!pointing.contains(FIRST), "{pointing}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_review_is_never_told_to_write_a_spec() {
        let (root, state, task) = world(&["api"]);
        on(&state);
        let review = Task {
            review: Some(crate::config::ReviewOf {
                repo: "acme/api".into(),
                number: 1,
                url: "https://github.com/acme/api/pull/1".into(),
                author: String::new(),
                head_sha: "abc".into(),
            }),
            ..task
        };
        assert_eq!(prompt_line(&state, &review), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_work_prompt_ticks_by_file_and_position_and_changes_the_spec_by_writing_it() {
        let (root, state, _) = world(&["api", "web"]);
        put(
            &spec_at(&root, "tasks.md"),
            "## api\n\n- [ ] 1. Add the route\n- [x] 2. Test it\n\n## web\n\n- [ ] 1. Call it (after api 1)\n",
        );
        let ask = |checkout: Option<&str>| work_prompt(&state, "t1", checkout).unwrap();

        let all = ask(None);
        assert!(all.contains(&format!("{}", spec_at(&root, "tasks.md").display())), "{all}");
        assert!(all.contains("3 steps"), "{all}");
        assert!(all.contains("`spec_task`") && all.contains("`file`") && all.contains("position"), "{all}");
        assert!(all.contains("`spec_write`"), "{all}");
        assert!(!all.contains("spec_propose") && !all.contains("repo `"), "{all}");

        let web = ask(Some("c-web"));
        assert!(web.contains("1 step") && web.contains("for `web`"), "{web}");
        assert!(web.contains("stop and say which step you are waiting for"), "{web}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_work_prompt_is_empty_when_the_spec_has_no_steps() {
        let (root, state, _) = world(&["api"]);
        put(&spec_at(&root, "requirements.md"), "# R\n");
        assert_eq!(work_prompt(&state, "t1", None).unwrap(), "");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_pull_request_section_reads_the_requirements_from_the_working_copy() {
        let (root, state, task) = world(&["api", "web"]);
        assert_eq!(pr_section(&state, &task), "", "no spec yet");
        put(
            &spec_at(&root, "requirements.md"),
            "## Requirements\n- R-1 [api]: WHEN a THE SYSTEM SHALL b.\n- R-2 [web]: WHEN c THE SYSTEM SHALL d.\n",
        );

        let md = pr_section(&state, &task);
        assert!(md.contains("## Spec of `api`: `specs/T-1/` in the repository"), "{md}");
        assert!(md.contains("- R-1: WHEN a THE SYSTEM SHALL b. — not checked"), "{md}");
        let (api, web) = md.split_once("## Spec of `web`").unwrap();
        assert!(!api.contains("R-2") && web.contains("R-2"), "{md}");
        std::fs::remove_dir_all(&root).ok();
    }
}
