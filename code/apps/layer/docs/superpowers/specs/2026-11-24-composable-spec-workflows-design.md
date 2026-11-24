# Composable spec workflows — design

Status: draft for approval · 2026-11-24 · replaces most of `docs/features.md` §19 (SPEC-1…25)

## Why

Layer's spec support (§19) is a Kiro-style pipeline that the app owns:
fixed `requirements|design|tasks.md`, drafts in `.spec-drafts/`, an
approve-then-commit gate, and anything an agent writes into the files is
flagged as "changed outside the app" (SPEC-22). Agents may change the spec
only through `spec_propose` with a whole file's text (SPEC-24).

The user's problems with it:

- Agents cannot easily read or edit the spec. The design treats an agent's
  edit as suspect.
- The spec cannot be split or shaped the user's way. The format is
  hard-coded Kiro.
- The user works with other spec toolsets (OpenSpec, superpowers) and wants
  Layer to host them, including their skills, for every agent CLI it runs.

## Goals

1. **Files are the record.** The spec is plain Markdown files at the task
   root. Agents and the user read and write them freely. Layer shows them
   and re-reads them when they change. No approval gate, no
   "changed outside the app".
2. **Pluggable toolsets.** A toolset (Kiro, OpenSpec, superpowers, custom)
   is a small manifest: where its files live, which hold checkbox steps,
   how to install it, what the root `AGENTS.md` says about it.
3. **Configurable at the task root and per repository.** The task picks a
   toolset. Each repository says which toolset it prefers and how the spec
   is committed into it.
4. **Install per task.** Creating a task installs its toolset (CLI files,
   skills) into the task folder for every agent CLI Layer can launch. Nothing
   is installed globally.
5. **Clear instructions.** The generated task-root `AGENTS.md`/`CLAUDE.md`
   says where the spec is, how the toolset works, how to tick steps, and the
   multi-repo conventions.
6. **MCP editing.** Agents get `spec_read`, `spec_write`, `spec_task`,
   `spec_commit` on the `villain-layer` MCP server, as an alternative to
   plain file access.
7. **Tab order.** Agents → Spec → Diff → Pull requests. A task opens on
   Agents.

## Non-goals

- A new Layer CLI binary. Agents use files or MCP. (My call: a CLI adds a
  second auth path for no gain over files.)
- Separate Spec and Build tabs with their own panes. The user decided
  against a UI split; only the tab order changes.
- Global installs of any toolset, or changes to the user's own
  `~/.claude`, `~/.codex`, etc.
- Editing toolset content (Layer does not rewrite OpenSpec's or
  superpowers' prompts or skills).
- Changes to the `vibe` repository. It may adopt parts of this later, on
  request.

## Anti-goals (would be a failure even if it works)

- Layer overwriting or "putting back" text an agent wrote.
- A toolset install that writes into a repository's worktree without the
  user asking (it would show up in Diff and in commits).
- A watcher or poll that makes the window slow (`AGENTS.md` rule: push,
  don't poll; nothing that waits on the main thread or the async runtime).
- Silent network fetches of unpinned code.

## Constraints from the codebase

- Nothing that waits runs on the main thread or the async runtime:
  `commands::blocking` / `off_runtime` (guard).
- New config fields need `#[serde(default)]`; `config.update` is not
  reentrant.
- Shared repo settings sync through `config/shared.rs` (explicit key
  allowlist), `workspace/mirror.rs`, `crates/domain` + a migration +
  `bun run gen:types`.
- Only `git.rs`/`git/` runs git; commits of spec files go through
  `git::commit_only`.
- MCP tools follow `docs/recipes.md` "Add an MCP tool"; the docs-sync guard
  requires the §12 table row. Tauri commands follow "Add a Tauri command".
- Generated task-folder files must be listed in `GENERATED_FILES`
  (`commands/panes.rs`) so Clean up removes them.
- `docs/features.md` is the contract: §19 is rewritten in the same commits
  as the code.

## Approaches considered

1. **Toolset manifests (chosen).** Each toolset is a data file shipped with
   Layer; custom ones live in settings. Generic code reads the manifest for
   paths, step files, install steps and the `AGENTS.md` paragraph. Kiro-only
   extras hang off the Kiro manifest.
2. **One Rust module per toolset.** Easiest place for Kiro extras, but every
   new toolset needs a Layer release and the module count grows.
3. **Toolsets do everything, Layer only shows Markdown.** Least code, but
   loses step progress, `spec_task` and the per-repository split.

The user picked 1 after confirming that checkbox steps are common to all
three toolsets, so one generic step parser covers them once the manifest
names the step files.

## The design

### 1. Where the spec lives

- The working copy is `<task folder>/spec/`, laid out the toolset's way
  (paths below are relative to it). `spec/` is a **mirror of a repository
  root**: a file at `spec/openspec/changes/x/tasks.md` is committed to
  `openspec/changes/x/tasks.md` in a repository. Only Kiro relocates, into
  `specs/{task}/` (its `commit_to`). (Builder check; my call.)
- Toolset tools run with `spec/` as their working directory: the context
  file tells agents to run `openspec …` from `<task>/spec`, and gives
  superpowers' save locations as absolute paths under
  `<task>/spec/docs/superpowers/` so a skill never writes into a
  repository's worktree. (Builder check; my call.)
- `spec/` starts empty apart from what the install writes. It is not
  seeded from a repository's existing `openspec/specs/` or
  `docs/superpowers/`. (My call: with several repositories there is no one
  right seed.)
- It is the only place agents and the user edit. Agents in any repository
  of the task reach it by absolute path (the context file names it) or MCP.
- When a task is deleted or cleaned up, its `spec/` is first copied to
  `<config folder>/specs/<task>/`, so a spec never committed (or in a
  repository with `spec_commit = none`) outlives the task on this Mac, as
  SPEC-3 promised. (Builder check; my call.)
- A task whose root is itself a checkout (older single-repo tasks, no task
  folder) uses `<config folder>/specs/<repository>/<task>/` as the working
  copy, as SPEC-3's fallback does today.
- The task folder is not in git and is deleted with the task. **Commit
  spec** (section 6) is how the spec reaches the repositories.

### 2. Toolset manifest

A toolset is a TOML table. Layer ships three; settings may add more
(`[toolsets.<id>]` in the app config, my call on the location).

```toml
[toolsets.openspec]
name    = "OpenSpec"
version = "1.14.3"                       # pinned; shown in the UI
files   = ["openspec/**/*.md"]           # what the Spec tab lists
steps   = ["openspec/changes/*/tasks.md"]
install = [
  { run = ["npx", "-y", "@fission-ai/openspec@{version}", "init", "--tools", "{openspec_tools}", "."], cwd = "spec" },
]
commit_to = "openspec"                   # default folder inside a repository
agents_md = "toolsets/openspec.md"       # paragraph for the root AGENTS.md

[toolsets.superpowers]
name    = "superpowers"
version = "7.0.2"
files   = ["docs/superpowers/**/*.md"]
steps   = ["docs/superpowers/plans/*.md"]
install = [
  { fetch = "https://github.com/obra/superpowers", ref = "v{version}", cache = true },
  { skills = "skills/*", to = "{skill_dirs}" },
]
commit_to = "docs/superpowers"
agents_md = "toolsets/superpowers.md"

[toolsets.kiro]
name    = "Kiro (Layer)"
files   = ["requirements.md", "bugfix.md", "design.md", "tasks.md"]
steps   = ["tasks.md"]
install = []
commit_to = "specs/{task}"
agents_md = "toolsets/kiro.md"
extras  = ["draft", "check", "questions"]
```

- `files` and `steps` are globs relative to `spec/`. A path with `..`, an
  absolute path or a symlink out of `spec/` is refused.
- `steps` must be a subset of `files`.
- Install step kinds, closed set:
  - `run`: argv (never a shell string), cwd `spec` or `task`, a timeout,
    output kept in the task's install log.
  - `fetch`: a git URL at a pinned `ref`, cloned once into
    `<config folder>/toolsets/<id>/<version>/` and reused.
  - `skills`: copy skill folders from the fetched copy into every skill
    directory of the CLIs this machine has set up (`{skill_dirs}`: today
    `.claude/skills/` for Claude Code and `.agents/skills/` for Codex and
    OpenCode, both in the task folder).
- Placeholders: `{version}`, `{task}`, `{skill_dirs}`, `{openspec_tools}`
  (the OpenSpec tool ids of the agent CLIs configured in Layer, e.g.
  `claude,codex`).
- `extras` turns on the Kiro-only features (section 9). Other toolsets get
  none.
- A custom toolset's `run` steps execute what the user wrote; the Settings
  UI says so when one is added.

### 3. Configuration

- **App default toolset**: a setting (Settings → Code). Default `kiro`, so
  existing users see no change in format.
- **Per repository** (Repos view, shared with the team like `spec_folder`
  today):
  - `spec_toolset`: the toolset this repository prefers, or none.
  - `spec_commit`: `whole` | `own` | `none` (section 6). Replaces
    `specs_in_app` (`specs_in_app = true` migrates to `none`).
  - `spec_folder`: where in the repository the spec goes; default the
    toolset's `commit_to`. Kept, re-meant.
- **Which wins**: the task's toolset decides everything for the task
  (layout, steps, install). A repository's `spec_toolset` is only the
  default Start work offers. A repository naming a toolset this Mac does not
  have (a teammate's custom one) is ignored for that default, with a note
  in the Repos view. `spec_folder` applies to Kiro only, since other
  toolsets mirror the repository root. (Builder check; my call.)
- **Spec first** (app setting, Settings → Code, default off): when on,
  every agent Layer starts is told to write or update the spec in
  `spec/` before it changes any code, then implement from it. Start work's
  "Write a spec first" is on and cannot be turned off for a launch.
  Instructions only: nothing blocks an agent that ignores them. (User's
  choice.)
- **Per task**: the toolset is fixed when the task is made. Start work
  offers it, defaulting to the toolset all the task's repositories prefer,
  else the app default. It is stored with the task. Changing it later is a
  menu item that installs the new toolset; existing files stay.

### 4. Install on task creation

- When Layer creates a task folder (Start work, a new task, `add_repo`
  adding the first repository), it runs the toolset's install steps in the
  background (`off_runtime`), after the worktrees exist.
- Progress and failure show on the task (a line on the Spec tab and a
  status dot). A failure does not stop the task or its agents. **Retry
  install** re-runs it.
- Installs write only into the task folder (`spec/`, `.claude/skills/`,
  `.agents/skills/`, files the toolset's own installer writes there).
  Every path written is recorded, so Clean up removes exactly those
  (`GENERATED_FILES` gains the record file).
- Nothing touches the repositories' worktrees.
- **Must verify (builder):** an agent started inside one repository
  (`scope = checkout`) must still see the task folder's
  `.claude/skills/` and `.agents/skills/`. If a CLI only reads skills from
  its own working directory, Layer starts that CLI with the task folder as
  an added directory, or links the skill folders into the agent's cwd's
  ignored config. The builder checks each CLI and records the outcome in
  §19's known gaps.

### 5. Steps and ticking

- One generic parser reads `- [ ]` / `- [x]` items from the manifest's
  `steps` files. It reuses `spec::steps`, `tick` and `keep_ticks`.
- **Which repository a step belongs to**: the nearest heading above it
  (any level) whose text is exactly a repository's folder name or carries
  a `[repo]` tag (`## 2. API [api]`), or a `[repo]` tag on the step's own
  line. A heading that names no repository does not change ownership. A
  step with no owner belongs to all repositories. So OpenSpec's
  `## 1. Setup` and superpowers' `### Task 3` keep working; splitting by
  repository needs the tag, which the multi-repo conventions ask for.
- **Addressing**: a step is its file plus its position (1-based, counting
  every checkbox in the file, nested ones included). The number the text
  shows (`1.1`, `Step 2`, `3.`) is displayed but never used to address it,
  since toolsets number differently and restart per section.
- `spec_task` (MCP) ticks by file + step number (file optional when there
  is one step file). A tick made by editing the file counts the same.
- The Spec tab shows per repository: ticked / total, from the files.

### 6. Commit spec

- **Commit spec** (Spec tab button, `spec_commit` MCP tool) copies the
  working copy into each repository, per that repository's `spec_commit`.
  It writes only files that exist in `spec/` and match `files`, at their
  mirrored path, and never deletes a file in a repository: a repository's
  existing `openspec/specs/` or older plans are left as they are.
  (Builder check; my call.)
  - `whole`: every file under `spec/` that matches `files`, into
    `spec_folder`.
  - `own`: same, but each step file keeps only the steps that belong to
    this repository (section 5), plus everything above the first `## <repo>`
    heading. Non-step files go whole.
  - `none`: nothing.
- It commits each repository with `git::commit_only` (only the spec's
  paths), as the user, message as SPEC-6 today
  ("Update the spec for ACME-123." or the conventional form).
- A repository it could not write or commit in is named with why; the
  others keep theirs.
- Open pull requests (PR-1) runs Commit spec first when the working copy
  differs from what was last committed, and says so.
- `spec_commit` over MCP needs `confirm: true` (my call: it writes git
  history in repositories the agent may not be working in).

### 7. The Spec tab

- Second tab, after Agents. Lists the toolset's `files` as a tree, opens
  one in the editor, and saves on edit (the user's edits are plain file
  writes, like an agent's).
- Shows the toolset and its install state, steps per repository, and the
  last Commit spec per repository (commit, or "changed since").
- **Live**: Layer watches `<task>/spec/` of the task on screen with the
  `notify` crate (FSEvents), debounced, and emits `spec:changed`. Only one
  watcher, for the shown task; it stops when the task is hidden. Agent
  stop (existing `Watchers.tsx` trigger) also re-reads, so a task not on
  screen is current when shown.
- An editor with unsaved edits whose file changed on disk keeps the user's
  text and says the file changed, with **Load theirs**. No merge.
- Start agent leaves the Spec tab. The launch dialog's prompt still points
  at the spec (section 8), and Kiro's work prompt (SPEC-12) becomes a
  launch-dialog preset.

### 8. Root `AGENTS.md` / `CLAUDE.md`

`task_context.rs` gains a **Spec** section, placed before the ticket:

- Where the working copy is (absolute path) and that it is the record:
  read it before work, edit it directly when the plan changes.
- The toolset's paragraph (`agents_md`), e.g. for OpenSpec: proposal →
  design → tasks per change, `openspec validate`, archive on done.
- How to tick: edit the checkbox or call `spec_task`; tick each step as soon
  as it is done, before the next.
- The MCP tools, one line each.
- With **Spec first** on: a line at the top of the file, and at the end of
  every opening prompt (`task_prompt`, launch dialog, handoffs, loops):
  "Write or update the spec in <abs>/spec before you change any code. Then
  implement it step by step, ticking each step." 
- **Multi-repository conventions** (only when the task has more than one
  repository), the suggested setup:
  - One spec for the task, never one per repository.
  - Requirements name the repositories that meet them (`[api, web]`).
  - Contracts between repositories (endpoints, events, schemas) are written
    once, in the design, each naming provider and users.
  - Step files group steps under `## <repo>` headings; a step waiting on
    another repository says `(after <repo> <n>)`.
  - Commit spec copies each repository its part; do not write spec files
    inside a repository's worktree.
- SPEC-10's "approved requirements carried above the ticket" is replaced by
  the first 8 KB of the toolset's entry file (Kiro: `requirements.md`;
  OpenSpec: the active change's `proposal.md`; superpowers: the newest
  design), cut with a pointer to the file.

### 9. MCP tools

| tool | does | confirm |
|---|---|---|
| `spec_read` | no `path`: list the spec's files with step counts; `path`: return the file's text | no |
| `spec_write` | write a file under `spec/` (whole text), creating folders; refused outside `spec/` or outside the toolset's `files` | no |
| `spec_task` | tick or untick a step (file?, step, done) | no |
| `spec_commit` | Commit spec (section 6), optional repository list | yes |

- `spec_propose` is removed. Its handler answers with a pointer to
  `spec_write` for one release (my call: running agents may still have it
  in context), then goes.
- Task resolution as today: `task_id` argument, else the calling pane's
  task.
- Writes go through a write-to-temp-and-rename under the existing spec
  mutex; `lstat` refuses symlinks (active lesson).

### 10. Kiro extras

For the Kiro toolset only (`extras`): Draft (SPEC-5), Apply answers and the
open-questions list (SPEC-19/20/21), Check against spec (SPEC-15/16). They
read and write the working copy's files directly. Drafting writes into the
file (with Undo restoring the previous text), not into `.spec-drafts/`.

### 11. What is removed

- `.spec-drafts/` and the approved record; approving (SPEC-6 approval
  commits become Commit spec); the forced order and out-of-date marking
  (SPEC-4, SPEC-7); changed outside the app, Put back, Open PRs' warning
  (SPEC-22, SPEC-23); `spec_propose` (SPEC-24); telling agents of an
  approval (SPEC-14); SPEC-25's merge of per-repository specs.
- Code: most of `commands/spec.rs` (approve), `commands/spec_outside.rs`,
  `SpecBanners.tsx` (outside/approval banners), `SpecStartAgent.tsx`.

### 12. Moving existing tasks

- A task with `.spec-drafts/approved/` and no `spec/`: on first open, the
  approved files are copied into `spec/` (Kiro layout), unapproved drafts
  over them when newer, and `.spec-drafts/` is left in place for one
  release.
- A task with only repositories' `specs/<task>/` folders: the first
  repository's copy (or each repository's `tasks.md` merged under
  `## <repo>` headings) becomes `spec/`.
- `SPEC.md` (SPEC-17): copied in as `requirements.md` when nothing else
  exists.
- Repositories' existing `specs/<task>/` folders are left alone; Commit
  spec overwrites them (Kiro's `commit_to` is the same path).

### 13. Tab order

`TABS = ["terminals", "spec", "diff", "pr"]` (labels Agents, Spec, Diff,
Pull requests). `select(task)` keeps opening on Agents. Start work with
"Write a spec first" (SPEC-18) opens on Spec; it no longer holds back the
agent, it starts the agent with a "write the spec first" prompt (my call:
agents are now the spec's authors).

### 14. Layer's own AGENTS.md

`code/apps/layer/AGENTS.md` gains a short "Specs and toolsets" note: where
toolset manifests live (`src-tauri/toolsets/`), the recipe for adding one,
that the working copy is `<task>/spec/`, and that Layer's own design docs
for this work live in `docs/superpowers/`.

## Testing

- Rust unit tests next to the code: manifest parsing and path validation
  (one valid manifest, one field mutated per test), step ownership by
  heading/tag, `own` filtering, `spec_write` path refusals (`..`, absolute,
  symlink), install step execution against a fake fetch cache, migration of
  each old layout, Commit spec with real temp git repos (reusing the
  `commands/spec.rs` test harness).
- MCP: `tools_list_names_the_danger_tier`, `danger_tools_advertise_confirm`
  cover `spec_commit`.
- UI: the mock harness (`src/__mock__/spec.ts`) gains an OpenSpec and a
  superpowers world; tab order and live reload are clicked through there.
- `bun run check` passes.

## Left to the builder

- Exact Settings and Repos view controls; copy text.
- Debounce interval; install timeout values.
- How custom toolsets are edited (raw TOML field vs form).
- The per-CLI skill-visibility check in section 4.
- OpenSpec's `archive` (merging a change's deltas into `openspec/specs/`)
  happens in each repository after merge, outside Layer. Listed as a known
  gap.
- Splitting the work into commits and a plan (next step:
  `writing-plans`).
