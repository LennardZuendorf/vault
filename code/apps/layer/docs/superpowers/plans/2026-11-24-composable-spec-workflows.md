# Composable spec workflows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Note:** The SPEC-n ids in this plan are those of `docs/features.md` §19 before it was rewritten (some of them from this branch's own earlier drafts), not its present ids.

**Goal:** Replace Layer's app-owned Kiro spec pipeline with a file-first spec working copy at the task root, driven by pluggable toolset manifests (Kiro, OpenSpec, superpowers, custom), installed per task, editable by agents over files or MCP.

**Architecture:** A toolset manifest (JSON data) tells generic Rust code where spec files live, which hold checkbox steps, how to install the toolset into the task folder, and what the root `AGENTS.md` says. The working copy is `<task>/spec/`, a mirror of a repository root; Commit spec copies it into each repository per that repository's setting. A `notify` watcher pushes `spec:changed` to the Spec tab.

**Tech Stack:** Tauri 2, Rust (serde_json, parking_lot, new: `notify`), React 19, Zustand store, Postgres domain crate + migration for shared repo fields.

**Spec:** `docs/superpowers/specs/2026-11-24-composable-spec-workflows-design.md`

## Global Constraints

- All paths below are relative to `code/apps/layer/`. Run every command from there.
- Follow `AGENTS.md` and `docs/recipes.md` ("Add a Tauri command", "Add an MCP tool", "Add a config field or a setting", "Splitting a file"). Guard rules apply.
- Blocking work (disk, git, processes, network) only inside `commands::blocking` or `commands::off_runtime`.
- Every new config field: `#[serde(default)]`. Never call a config accessor inside `config.update`.
- No `unwrap`/`expect`/`panic!` outside tests. Tests at the bottom of the file they test, named as sentences, real git in temp dirs.
- `docs/features.md` §19 changes in the same commit as the behaviour it describes. MCP tools get a §12 row (docs-sync guard). New events get a row in `docs/architecture.md`.
- Manifests are **JSON**, not TOML (deviation from the design's example: `config.json` is JSON and there is no `toml` dependency). Glob support is `*` and `**` only, hand-written; no new glob crate.
- One new dependency only: `notify` (FSEvents watcher). The PR says why: nothing in the tree watches files, and polling breaks the "push, don't poll" rule.
- Built-in toolset ids: `kiro`, `openspec`, `superpowers`. Pinned versions: OpenSpec `1.14.3` (`npx -y @fission-ai/openspec@1.14.3 init --tools <ids> .`), superpowers `7.0.2` (git `https://github.com/obra/superpowers`, ref `v7.0.2`).
- Working copy: `<task.root>/spec/`; fallback for tasks with no folder of their own: `<config folder>/specs/<repository>/<task>/`; archive on delete: `<config folder>/specs/<task>/`.
- Commit messages: Layer style, a sentence saying what is better for the user. Commit only when the user has asked for commits in the session.
- `spec_commit` over MCP requires `confirm: true` (add to `DANGER`).

## Review Focus

- **A path that escapes `spec/`** (`../x`, `/etc/x`, a symlinked folder inside `spec/`) through `spec_write`, Commit spec or the install's `skills` copy: refused, nothing written. Tests in Tasks 3 and 4.
- **Commit spec onto a repository that already has `openspec/specs/` or older plans:** files not in `spec/` stay byte-identical, nothing deleted. Test in Task 5.
- **An agent and the user editing the same file:** the editor keeps unsaved text and offers Load theirs; the agent's write is never overwritten silently. Test in Task 10 (store logic) and mock click-through.
- **Install with no network or no `npx`:** task and agents still start; the Spec tab shows the failure and Retry. Test in Task 4.
- **An old task opened after upgrade** (`.spec-drafts/approved/` or `specs/<task>/` in worktrees, or `SPEC.md`): its approved text appears in `spec/` without loss. Tests in Task 3.

---

### Task 1: Toolset manifests

**Files:**
- Create: `src-tauri/src/spec/toolset.rs`, `src-tauri/src/spec/glob.rs`
- Create: `src-tauri/toolsets/kiro.json`, `openspec.json`, `superpowers.json`
- Create: `src-tauri/toolsets/kiro.md`, `openspec.md`, `superpowers.md` (the `AGENTS.md` paragraphs)
- Modify: `src-tauri/src/spec.rs` (add `pub mod toolset; pub mod glob;`)

**Interfaces:**
- Produces:
  - `pub struct Toolset { id: String, name: String, version: Option<String>, files: Vec<String>, steps: Vec<String>, install: Vec<InstallStep>, commit_to: Option<String>, agents_md: String, extras: Vec<Extra> }` (serde, `#[serde(default)]` on optional fields)
  - `pub enum InstallStep { Run { run: Vec<String>, cwd: RunIn, timeout_secs: Option<u64> }, Fetch { fetch: String, r#ref: String }, Skills { skills: String } }` (untagged), `pub enum RunIn { Spec, Task }`, `pub enum Extra { Draft, Check, Questions }`
  - `pub fn builtin() -> Vec<Toolset>` (from `include_str!`), `pub fn find(id: &str, custom: &[Toolset]) -> Option<Toolset>` (custom overrides built-in with same id)
  - `pub fn validate(t: &Toolset) -> Result<(), String>`
  - `pub fn expand(arg: &str, vars: &Vars) -> String` with `pub struct Vars { version, task, openspec_tools, skill_dirs }`
  - `glob::matches(pattern: &str, rel: &str) -> bool`, `glob::is_safe_rel(rel: &str) -> bool`

- [ ] **Step 1: Write failing tests** in `spec/toolset.rs` and `spec/glob.rs`:
  - `every_builtin_toolset_is_valid` — `builtin()` has ids `kiro, openspec, superpowers`, each passes `validate`.
  - `a_step_glob_outside_the_file_globs_is_refused` — clone openspec, set `steps = ["other/*.md"]`, `validate` errs.
  - `a_glob_with_dotdot_or_a_leading_slash_is_refused` — `files = ["../x"]` and `["/x"]` err.
  - `a_run_step_with_an_empty_argv_is_refused`.
  - `a_custom_toolset_overrides_the_builtin_with_its_id`.
  - `placeholders_expand_in_run_arguments` — `expand("@fission-ai/openspec@{version}", vars)` → `@fission-ai/openspec@1.14.3`.
  - `double_star_matches_any_depth_and_star_one_segment` — `matches("openspec/**/*.md","openspec/changes/a/specs/x/spec.md")` true; `matches("docs/superpowers/plans/*.md","docs/superpowers/plans/a/b.md")` false.
  - `unsafe_relative_paths_are_rejected` — `is_safe_rel` false for `""`, `"../a"`, `"a/../../b"`, `"/a"`, `"a\\b"`.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec::toolset spec::glob` — FAIL (module missing).
- [ ] **Step 3: Implement.** Manifest JSON mirrors the design's TOML tables. `validate` accepts a `steps` pattern when some `files` pattern matches it with each `*` replaced by `x` (`matches(files_j, sample)`). Paragraph files are loaded with `include_str!` into `agents_md`.
- [ ] **Step 4: Run** same command — PASS.
- [ ] **Step 5: Commit** "Describe spec toolsets as data."

### Task 2: Steps owned by repositories

**Files:**
- Create: `src-tauri/src/spec/owned.rs`
- Modify: `src-tauri/src/spec.rs` (mod line; reuse `list_item`, `checkbox`, `outside_fences` by making them `pub(super)`)

**Interfaces:**
- Consumes: `spec::checkbox`, `spec::list_item`, `spec::outside_fences`.
- Produces:
  - `pub struct OwnedStep { pub position: u32, pub label: String, pub text: String, pub done: bool, pub repos: Vec<String> }` (`repos` empty = every repository; `label` = the number as written, `""` if none)
  - `pub fn owned_steps(text: &str, folders: &[String]) -> Vec<OwnedStep>`
  - `pub fn tick_at(text: &str, position: u32, done: bool) -> Option<String>`
  - `pub fn own_part(text: &str, folder: &str, folders: &[String]) -> String`

- [ ] **Step 1: Write failing tests:**
  - `a_heading_named_after_a_repository_owns_the_steps_under_it` — `"## api\n- [ ] a\n## web\n- [ ] b"`, folders `[api,web]` → repos `[api]`, `[web]`.
  - `a_bracket_tag_on_a_heading_owns_its_steps` — `"## 2. API [api]\n- [ ] 2.1 x"` → `[api]`, label `"2.1"`.
  - `a_heading_naming_no_repository_keeps_the_owner_above_it` — `"## api\n### Task 3\n- [ ] x"` → `[api]`.
  - `openspec_sections_leave_steps_owned_by_everyone` — `"## 1. Setup\n- [ ] 1.1 Add x"` → repos `[]`.
  - `a_tag_on_the_step_line_wins_over_the_heading`.
  - `positions_count_nested_checkboxes_in_file_order` — superpowers-style nested `- [ ] **Step 1**` under `### Task 1` and `### Task 2` → positions 1..4.
  - `ticking_by_position_changes_only_that_box` — `tick_at(text, 3, true)` changes the third `[ ]` only; out of range → `None`.
  - `checkboxes_inside_code_fences_are_not_steps`.
  - `own_part_keeps_the_preamble_and_only_this_repositorys_sections` — preamble above first repo heading kept; `## web` section dropped for `api`; unowned steps kept.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec::owned` — FAIL.
- [ ] **Step 3: Implement** with one pass over lines tracking a heading stack `(level, owner)`; a heading's owner is the folder it names exactly (case-insensitive) or its `[tag]`, else inherited from the nearest enclosing owned heading.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** "Read checkbox steps of any spec toolset, per repository."

### Task 3: Settings, the working copy and moving old specs

**Files:**
- Modify: `src-tauri/src/config.rs` (`AppConfig.default_toolset: Option<String>`, `AppConfig.toolsets: Vec<Toolset>`; `Task.toolset: Option<String>`)
- Modify: `src-tauri/src/config/project.rs` (add `spec_toolset: Option<String>`, `spec_commit: SpecCommit`; keep `spec_folder`; remove `specs_in_app` with a serde alias migration: `specs_in_app = true` reads as `SpecCommit::None`)
- Modify (shared field plumbing, per recipe step 8): `src-tauri/src/config/shared.rs` (allowlist: replace `"specs_in_app"` with `"spec_toolset","spec_commit"`), `src-tauri/src/workspace/mirror.rs`, `crates/domain/src/repository.rs`, `migrations/0007_spec_toolsets.sql`, `packages/types` (`bun run gen:types`), `web/src/sync.ts`, `web/src/repositories.tsx`, every `Project { … }` literal listed by the compiler.
- Create: `src-tauri/src/commands/spec_copy.rs`
- Modify: `src-tauri/src/commands/cleanup.rs` / task delete path (archive), `src-tauri/src/commands/panes.rs` (`GENERATED_FILES`: add `"spec"`-is-not-generated note; add `INSTALL_RECORD` const from Task 5 later)

**Interfaces:**
- Produces:
  - `pub enum SpecCommit { #[default] Whole, Own, None }` (serde lowercase)
  - `pub(crate) fn toolset_of(state: &AppState, task: &Task) -> Toolset` (task's id → `find`, else `default_toolset`, else `kiro`)
  - `pub(crate) fn spec_dir(state: &AppState, task: &Task) -> PathBuf`
  - `pub(crate) fn list_files(dir: &Path, t: &Toolset) -> Result<Vec<String>>` (relative, sorted, symlinks skipped)
  - `pub(crate) fn read_file(dir: &Path, t: &Toolset, rel: &str) -> Result<String>`
  - `pub(crate) fn write_file(dir: &Path, t: &Toolset, rel: &str, text: &str) -> Result<()>` (refuse unsafe rel, non-matching `files`, any symlinked ancestor via `lstat`; temp file opened `create_new` then rename; one `parking_lot::Mutex`)
  - `pub(crate) fn move_old_spec(state: &AppState, task: &Task) -> Result<bool>` (idempotent; runs when `spec/` is absent)
  - `pub(crate) fn archive_spec(state: &AppState, task: &Task) -> Result<()>`

- [ ] **Step 1: Write failing tests** (in `spec_copy.rs`, temp dirs):
  - `specs_in_app_true_reads_as_commit_none` (config.rs test).
  - `a_task_without_a_toolset_uses_the_app_default_then_kiro`.
  - `writing_outside_the_spec_folder_is_refused` — `../x.md`, `/tmp/x.md`.
  - `writing_through_a_symlinked_folder_is_refused`.
  - `writing_a_file_the_toolset_does_not_list_is_refused` — openspec, `notes.txt`.
  - `a_write_creates_missing_folders_and_replaces_whole_text`.
  - `approved_drafts_move_into_spec` — `.spec-drafts/approved/requirements.md` + `api/tasks.md` → `spec/requirements.md`, `spec/tasks.md` with `## api` section.
  - `repository_copies_move_into_spec_when_nothing_was_approved` — two worktrees with `specs/<task>/tasks.md` → merged under `## api`/`## web`.
  - `an_old_spec_md_becomes_requirements` — only `SPEC.md` present.
  - `moving_twice_changes_nothing`.
  - `deleting_a_task_keeps_its_spec_in_the_config_folder`.
  - Shared sync: extend the existing allowlist test fixture in `config/shared.rs` with `spec_toolset`/`spec_commit`; `crates/domain` repository test for validation (`spec_commit` one of `whole|own|none`).
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec_copy config` — FAIL.
- [ ] **Step 3: Implement** the fields, migration (`ALTER TABLE repositories ADD COLUMN spec_toolset text, ADD COLUMN spec_commit text NOT NULL DEFAULT 'whole'`; `UPDATE … SET spec_commit='none' WHERE specs_in_app`; drop `specs_in_app`), and `spec_copy.rs`. Replace `set_project_specs(project_id, in_app, folder)` with `set_project_specs(project_id, toolset: Option<String>, commit: SpecCommit, folder: Option<String>)`; update `api.ts`, `types.ts`.
- [ ] **Step 4: Run** `cargo test -p villain-layer --lib` and `bun run db && cargo test -p layer-domain -p layer-sync -p layer-server` — PASS. `bun run gen:types` leaves a clean diff on second run.
- [ ] **Step 5: Commit** "Keep a task's spec as files at the task root."

### Task 4: Install a toolset into a task

**Files:**
- Create: `src-tauri/src/commands/spec_install.rs`
- Modify: `src-tauri/src/commands/tasks.rs` (after worktrees exist, and when the first repository is added: spawn install via `off_runtime`), `src-tauri/src/commands/panes.rs` (`GENERATED_FILES` gets `INSTALL_RECORD`; `remove_generated` also removes every path the record lists), `lib.rs`, `src/lib/api.ts`, `src/lib/types.ts`, `src/__mock__/boot.ts`, `docs/architecture.md` (event row)

**Interfaces:**
- Consumes: `Toolset`, `InstallStep`, `expand`, `Vars` (Task 1); `spec_dir`, `toolset_of` (Task 3).
- Produces:
  - `pub(crate) const INSTALL_RECORD: &str = ".spec-install.json"` (in the task folder: `{ toolset, version, ok, error, log_tail, written: [rel paths] }`)
  - `pub(crate) fn install(state: &AppState, task: &Task, runner: &dyn Runner) -> InstallState`
  - `pub(crate) trait Runner { fn run(&self, argv: &[String], cwd: &Path, timeout: Duration) -> Result<String>; fn fetch(&self, url: &str, r#ref: &str, into: &Path) -> Result<()>; }` — real impl uses `std::process::Command` with the login env (`shellenv`) for `run`, and a new named function `git::clone_at(url, r#ref, into)` in `git.rs` for `fetch`.
  - `pub struct InstallState { pub toolset: String, pub version: Option<String>, pub ok: bool, pub error: Option<String>, pub running: bool }`
  - `#[tauri::command] pub async fn install_spec_toolset(app, task_id: String) -> Result<InstallState>` (Retry)
  - Event `spec:install` with `{ taskId, state: InstallState }`.
  - Skill dirs: `.claude/skills` and `.agents/skills` under the task folder; `{openspec_tools}` = OpenSpec ids for the agent CLIs configured in `agents.rs` (`claude`→`claude`, `codex`→`codex`, `opencode`→`opencode`, `gemini`→`gemini`; others skipped).

- [ ] **Step 1: Write failing tests** with a fake `Runner`:
  - `installing_openspec_runs_its_pinned_init_in_the_spec_folder` — argv `["npx","-y","@fission-ai/openspec@1.14.3","init","--tools","claude,codex","."]`, cwd `<task>/spec`.
  - `installing_superpowers_copies_its_skills_into_every_skill_folder` — fake fetch creates `skills/brainstorming/SKILL.md`; both `.claude/skills/brainstorming/SKILL.md` and `.agents/skills/brainstorming/SKILL.md` exist; record lists them.
  - `a_fetched_copy_is_reused_for_the_next_task` — fetch called once for two tasks.
  - `a_failed_step_is_recorded_and_does_not_error_the_task` — runner errs; `ok=false`, `error` set, function returns.
  - `a_skill_folder_symlinked_out_of_the_cache_is_not_copied`.
  - `nothing_is_written_inside_a_repository_worktree` — after install, `git status --porcelain` in each checkout is empty.
  - `cleanup_removes_exactly_what_the_install_wrote` — a user file in `.claude/` survives.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec_install` — FAIL.
- [ ] **Step 3: Implement.** Cache at `<config folder>/toolsets/<id>/<version>/`. Timeout default 120 s. Keep last 4 KB of output in `log_tail`.
- [ ] **Step 4: Run** — PASS. `bun run guard` — PASS.
- [ ] **Step 5: Commit** "Install the task's spec toolset for every agent CLI."
- [ ] **Step 6: Manual check (record result in §19 known gaps):** start Claude Code and Codex with `scope = checkout` in a task with superpowers installed; ask each to list its skills. If a CLI does not see the task folder's skills, pass the task folder as an added directory in that CLI's launch args in `agents.rs` and re-check.

### Task 5: Commit spec

**Files:**
- Create: `src-tauri/src/commands/spec_commit.rs`
- Modify: `src-tauri/src/commands/open_prs.rs` (replace `commit_ticks` call with `commit_spec` when changed since last commit), `lib.rs`, `api.ts`, `types.ts`, mock

**Interfaces:**
- Consumes: `owned_steps`, `own_part` (Task 2); `spec_dir`, `list_files`, `toolset_of`, `SpecCommit` (Task 3); `git::commit_only`; `RepoResult` (`commands/diff.rs`); `spec::folder_name`; the conventional-message helper from `commands/spec.rs` (move it into `spec_commit.rs`).
- Produces:
  - `pub(crate) fn target_path(t: &Toolset, task: &Task, project: &Project, rel: &str) -> Option<String>` — Kiro: `<spec_folder or "specs">/<folder_name(branch)>/<rel>`; others: `rel` unchanged.
  - `pub(crate) fn commit_spec(state: &AppState, task: &Task, only: Option<&[String]>) -> Vec<RepoResult>`
  - `pub(crate) fn changed_since_commit(state: &AppState, task: &Task) -> Vec<String>` (repository folders whose committed copy differs from `spec/`)
  - `#[tauri::command] pub async fn commit_spec_cmd(app, task_id: String, repos: Option<Vec<String>>) -> Result<Vec<RepoResult>>` (registered as `commit_spec`)

- [ ] **Step 1: Write failing tests** (real git, reuse the `commands/spec.rs` test harness — move it to a shared `#[cfg(test)] mod harness` if needed):
  - `whole_copies_every_spec_file_to_its_mirrored_path` — openspec `spec/openspec/changes/a/tasks.md` → repo `openspec/changes/a/tasks.md`.
  - `kiro_files_go_under_the_tasks_spec_folder` — `specs/ACME-1-x/requirements.md`.
  - `own_keeps_only_this_repositorys_steps`.
  - `none_writes_nothing`.
  - `existing_repository_spec_files_are_never_deleted_or_touched` — pre-existing `openspec/specs/auth/spec.md` byte-identical after commit; `git show --stat HEAD` lists only spec paths.
  - `only_spec_paths_are_committed_while_an_agent_has_changes_staged`.
  - `a_repository_that_fails_is_named_and_the_others_commit`.
  - `a_conventional_repository_gets_a_conventional_message` — `docs: update the spec for ACME-1`.
  - `nothing_changed_means_no_commit`.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec_commit` — FAIL.
- [ ] **Step 3: Implement.** Message: "Update the spec for {key}." / `docs: update the spec for {key}`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** "Commit the task's spec into each repository on demand."

### Task 6: MCP tools

**Files:**
- Modify: `src-tauri/src/mcp.rs` (tool defs in a new `spec_tools()` list, `call()` arms, `DANGER += "spec_commit"`), `src-tauri/src/commands/spec_work.rs` (`tick_step` rewritten over `owned_steps`/`tick_at`), `src-tauri/src/commands/spec_outside.rs` (keep only the `spec_propose` stub until Task 9 deletes the file), `docs/features.md` §12 table

**Interfaces:**
- Consumes: Tasks 2, 3, 5.
- Produces (MCP):
  - `spec_read { task_id?, path? }` → no path: `[{ path, steps_done, steps_total }]`; path: file text.
  - `spec_write { task_id?, path, text }` → `"wrote <path>"`; emits `spec:changed`.
  - `spec_task { task_id?, file?, step, done=true }` → the step as JSON; `file` optional when exactly one steps file exists; `step` is the position.
  - `spec_commit { task_id?, repos?, confirm }` → `RepoResult` rows.
  - `spec_propose` → error text: "spec_propose is gone: write the file with spec_write, or edit it under <spec dir>."
- Produces (Rust): `pub(crate) fn tick_step(state, task_id: &str, file: Option<&str>, position: u32, done: bool) -> Result<OwnedStep>`

- [ ] **Step 1: Write failing tests:** `spec_tools_are_listed_with_their_schemas`, existing `tools_list_names_the_danger_tier` and `danger_tools_advertise_confirm` now include `spec_commit`; `spec_write_from_a_pane_writes_into_its_tasks_spec_folder`; `spec_task_without_a_file_is_refused_when_two_step_files_exist`; `spec_propose_points_at_spec_write`; `concurrent_ticks_of_one_file_all_land` (adapt existing test).
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib mcp spec_work` — FAIL.
- [ ] **Step 3: Implement** per recipe; §12 rows for the four tools, `spec_propose` row removed.
- [ ] **Step 4: Run** tests and `bun run guard` (docs-sync) — PASS.
- [ ] **Step 5: Commit** "Let agents read, write, tick and commit the spec over MCP."

### Task 7: The context file

**Files:**
- Modify: `src-tauri/src/commands/task_context.rs` (`spec_section` rewritten; spec-first line), `src-tauri/src/commands/spec_work.rs` (`prompt_line` adds the spec-first line to every opening prompt; `pr_section` reads the working copy), `src-tauri/src/config.rs` (`spec_first`), `src-tauri/toolsets/*.md`

**Interfaces:**
- Consumes: `toolset_of`, `spec_dir`, `list_files`, `owned_steps`; new field `AppConfig.spec_first: bool` (`#[serde(default)]`, added in this task, mirrored in `types.ts`, set through `set_ui_prefs` or a `set_spec_first(on: bool)` command per the settings recipe).
- Produces: `fn spec_section(t: &Toolset, dir: &Path, folders: &[String], entry: Option<(String, String)>) -> String`; `pub(crate) fn entry_file(t: &Toolset, dir: &Path) -> Option<(String, String)>` (Kiro: `requirements.md` or `bugfix.md`; OpenSpec: newest `openspec/changes/*/proposal.md` by mtime; superpowers: newest `docs/superpowers/specs/*.md`; cut at 8 KB with a pointer).

- [ ] **Step 1: Write failing tests:**
  - `the_context_names_the_spec_folder_by_absolute_path_and_the_toolset`.
  - `multi_repository_conventions_appear_only_with_two_or_more_repositories` — strings `One spec for the task`, `[api, web]`, `## <repo>` / `[repo]`, `(after <repo> <n>)`.
  - `openspec_is_told_to_run_from_the_spec_folder` — contains `cd <abs>/spec` (or "run `openspec` from").
  - `superpowers_save_paths_point_into_the_spec_folder` — absolute `<abs>/spec/docs/superpowers/specs/` and `/plans/`.
  - `the_entry_file_is_cut_at_8_kb_with_a_pointer`.
  - `the_context_lists_spec_read_spec_write_spec_task_spec_commit`.
  - `spec_first_puts_its_line_at_the_top_of_the_context_and_the_end_of_the_opening_prompt` — `AppConfig.spec_first = true`; context starts its body with `Write or update the spec in <abs>/spec before you change any code.`; `task_prompt` ends with the same line.
  - `spec_first_off_adds_nothing`.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib task_context` — FAIL.
- [ ] **Step 3: Implement.** Section placed before the ticket, as today.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** "Tell agents where the spec is and how its toolset works."

### Task 8: Watch the spec folder

**Files:**
- Create: `src-tauri/src/commands/spec_watch.rs`
- Modify: `src-tauri/Cargo.toml` (`notify = "8"`), `lib.rs`, `api.ts`, `src/Watchers.tsx`, `docs/architecture.md`

**Interfaces:**
- Produces: `#[tauri::command] pub async fn watch_spec(app, task_id: Option<String>) -> Result<()>` — replaces any current watcher (one at a time); `None` stops. Debounce 200 ms; emits `spec:changed` with the task id. Ignores events for paths outside `spec/`.

- [ ] **Step 1: Write failing test:** `a_write_in_the_spec_folder_is_reported_once_after_the_debounce` (temp dir, channel sink instead of `AppHandle`; three writes in 50 ms → one notice); `watching_another_task_stops_the_first`.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec_watch` — FAIL.
- [ ] **Step 3: Implement**; `Watchers.tsx` calls `api.watchSpec(id)` when the Spec tab of a task is shown and `api.watchSpec(null)` when it is not.
- [ ] **Step 4: Run** — PASS; `bun run guard` (event-sync) PASS.
- [ ] **Step 5: Commit** "Show an agent's spec edits as they are written."

### Task 9: Remove the approval pipeline; Kiro extras write files

**Files:**
- Delete: `src-tauri/src/commands/spec_outside.rs`, `src/components/spec/SpecBanners.tsx`, `src/components/spec/SpecStartAgent.tsx`
- Modify: `src-tauri/src/commands/spec.rs` (remove `approve_spec`, `approve`, `approve_in`, `Notes`/`Proposal`/drafts state; keep `Check`/`Verdict` storage moved under `<task>/.spec-checks/`), `commands/spec_view.rs` (`read_spec` returns the new view; remove `save_spec_draft`), `commands/spec_draft.rs` (`draft_spec`, `apply_spec_answers`, `merge_specs` removed or rewired: Draft and Apply answers write the file via `write_file` and return the previous text for Undo; `merge_specs` deleted — moving old specs is Task 3), `commands/spec_work.rs` (remove `tell_spec_change`, `commit_ticks`), `lib.rs`, `api.ts`, `types-spec.ts`, `src/lib/specs.ts`, mock, `scripts/guard.ts` ceilings if files shrink
- Gate extras: `draft_spec`, `apply_spec_answers`, `check_spec` return an error unless `toolset_of(task).extras` contains the matching `Extra`.

**Interfaces:**
- Produces: `#[tauri::command] read_spec(task_id) -> SpecView` where `SpecView { toolset: Toolset (id, name, version, extras), dir: String, files: Vec<SpecFileView { path, text, steps: Vec<OwnedStep> }>, repos: Vec<RepoSpecView { folder, commit: SpecCommit, done: u32, total: u32, changed_since_commit: bool }>, install: Option<InstallState> }`; `#[tauri::command] write_spec_file(task_id, path, text)`.

- [ ] **Step 1: Write failing tests:** `read_spec_lists_the_toolsets_files_with_their_steps`; `drafting_is_refused_outside_kiro`; `a_kiro_draft_writes_the_file_and_returns_what_it_replaced`; `check_against_spec_reads_the_working_copy`.
- [ ] **Step 2: Run** `cargo test -p villain-layer --lib spec` — FAIL.
- [ ] **Step 3: Implement**; delete dead code everywhere in one change (dead-command guard).
- [ ] **Step 4: Run** `cargo test -p villain-layer --lib` and `bun run guard` — PASS.
- [ ] **Step 5: Commit** "Stop gating spec edits behind approval."

### Task 10: The Spec tab, settings and tab order

**Files:**
- Modify: `src/components/SpecView.tsx` (file tree + editor + steps per repository + install line + Commit spec button + Load theirs), `src/components/spec/SpecSide.tsx`, `src/components/spec/SpecQuestions.tsx` (Kiro only), `src/lib/specs.ts`, `src/store.ts` (`TABS = ["terminals","spec","diff","pr"]`), `src/App.tsx` (tab bar order), `src/components/tickets/StartWorkDialog.tsx` (toolset pick, default from repositories; "Write a spec first" starts the agent with the spec prompt and opens Spec), `src/components/RepoSettings.tsx` + `ReposView.tsx` (toolset, commit mode, folder for Kiro; note for an unknown toolset), `src/components/CodeSettings.tsx` (default toolset; "Always write a spec first" switch bound to `spec_first`, which also locks StartWorkDialog's "Write a spec first" on; custom toolsets as a JSON field with a warning that `run` steps execute as written), `src/__mock__/spec.ts` (Kiro, OpenSpec, superpowers worlds), `docs/testing.md`
- Create: `src/lib/spec-edit.ts` — `export function mergeDisk(editor: {text: string; dirty: boolean; base: string}, disk: string): { text: string; conflict: boolean }` (dirty and disk≠base → keep editor text, conflict true; clean → take disk)
- Test: `src/lib/spec-edit.test.ts`

- [ ] **Step 1: Write failing tests** `spec-edit.test.ts`: `a clean editor takes the file from disk`; `a dirty editor keeps its text and reports a conflict when the file changed`; `a dirty editor with an unchanged file reports no conflict`.
- [ ] **Step 2: Run** `bun test src/lib/spec-edit.test.ts` — FAIL.
- [ ] **Step 3: Implement** the helper and the UI. Use `ui-*` classes from `src/design/components.css`; read `studio/design/principles.md` and `behaviour.md` first.
- [ ] **Step 4: Run** `bun run check` — PASS. Click through in the mock harness (`bun run dev`, `/mock.html`): tab order, each toolset world, live update, conflict, Commit spec results per repository, install failure + Retry.
- [ ] **Step 5: Commit** "Put Agents first and show any toolset's spec in the Spec tab."

### Task 11: Rewrite §19 and finish

**Files:**
- Modify: `docs/features.md` §19 (new SPEC ids, including Spec first replacing SPEC-1…25: working copy, toolsets, configuration, install, steps, Commit spec, Spec tab, context file, MCP tools, Kiro extras, moving old specs, tab order; known gaps incl. OpenSpec archive outside Layer and the Task 4 Step 6 result), §13 settings table, §14 file table (`spec/`, `.spec-install.json`, `.spec-checks/`), `docs/architecture.md` events, `docs/recipes.md` ("Add a spec toolset"), `AGENTS.md` (short "Specs and toolsets" note: manifests in `src-tauri/toolsets/`, the recipe, working copy `<task>/spec/`, design docs in `docs/superpowers/`)

- [ ] **Step 1:** Write the docs. Every requirement names the code that meets it.
- [ ] **Step 2: Run** `bun run check` — PASS (full output kept for the verify gate).
- [ ] **Step 3: Commit** "Describe composable spec workflows."
