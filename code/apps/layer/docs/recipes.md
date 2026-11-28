# Recipes

Step by step, for the changes this codebase sees most. Each recipe lists
every place a change of that kind touches. Missing one is how the code gets
out of sync with itself. `bun run check` catches some of the misses (it
names the rule). The rest are on you.

- [Add a Tauri command](#add-a-tauri-command)
- [Add a config field or a setting](#add-a-config-field-or-a-setting)
- [Add a backend event](#add-a-backend-event)
- [Add a view, tab or dialog](#add-a-view-tab-or-dialog)
- [Add an agent CLI](#add-an-agent-cli)
- [Add an MCP tool](#add-an-mcp-tool)
- [Add a Jira, GitHub or Slack call](#add-a-jira-github-or-slack-call)
- [Add a git operation](#add-a-git-operation)
- [Add a spec toolset](#add-a-spec-toolset)
- [Fix a bug](#fix-a-bug)
- [Splitting a file](#splitting-a-file)

---

## Add a Tauri command

1. **Write it in `src-tauri/src/commands/<area>.rs`.** If it touches git,
   the disk, a child process or the keychain, it is `async` and the work
   goes through `blocking`:

   ```rust
   /// What it is for, and anything surprising about it — the doc comment
   /// sits directly above #[tauri::command].
   #[tauri::command]
   pub async fn task_thing(app: AppHandle, task_id: String) -> Result<Thing> {
       super::blocking(app, move |state| task_thing_inner(state, &task_id)).await
   }

   /// The body, callable from the MCP server and from tests.
   pub(crate) fn task_thing_inner(state: &AppState, task_id: &str) -> Result<Thing> {
       let task = state.config.task(task_id)?;
       // git::…, state.ptys.…
   }
   ```

   Use a plain `fn` only for work that is memory only: reading config,
   listing panes. An HTTP call is an `async fn` that awaits the client.
   Nothing waits in it except the request.
2. **Register it** in `generate_handler!` in `src-tauri/src/lib.rs`.
3. **Wrap it** in `src/lib/api.ts`. The argument names are the Rust
   parameter names in camelCase:

   ```ts
   taskThing: (taskId: string) => invoke<Thing>("task_thing", { taskId }),
   ```

4. **Mirror the return type** in `src/lib/types.ts`, with the Rust field
   names (snake_case).
5. **Answer it in the mock harness** (`src/__mock__/boot.ts`, `answer`) if
   the UI calls it on load or in the flow you will verify.
6. **Test the `_inner` function** at the bottom of its file, with real git
   in a temp dir if it runs git.
7. `bun run check`. The guard checks steps 2 and 3.

To remove a command, undo every step, in the same commit.

## Add a config field or a setting

1. **Add the field** to its struct in `src-tauri/src/config.rs`, with
   `#[serde(default)]` (or `#[serde(default = "yes")]` for a `true`
   default). A doc comment says what it controls, and why the default is
   what it is.
2. For a UI preference, add it to `UiPrefs`, to its `Default` impl, **and**
   to `set_ui_prefs` in `commands/settings.rs`, which rebuilds the struct
   field by field so each value can be clamped (the compiler asks for this
   one).
3. **Mirror it** in `src/lib/types.ts` (`UiPrefs`, `Settings`, …).
4. **Add the control** on the tab of what it changes (SET-6): each tab is
   a file of its own, `AppearanceSettings.tsx`, `AgentSettings.tsx` and so
   on, and `useUiPrefs` saves it. For a switch:

   ```tsx
   <Switch
     label="Say what it does"
     detail="What turning it on changes, and why someone would not want it."
     checked={ui.the_field}
     onChange={(v) => void saveUi({ the_field: v }, "the-place")}
   />
   ```

5. **Add it to the settings table** in `docs/features.md` (§13), with its
   default. Then search the file for the behaviour it switches: a
   requirement or table elsewhere (§11 Notifications, say) may describe it,
   and that is the contract to change first.
6. **Update the mock world's** `ui` in `src/__mock__/world.ts`. `tsc` will
   tell you.
7. A field that holds data (not a preference) and is later removed drops
   that key from users' configs on the next write. Removing one is a
   decision, not a cleanup.
8. **A field of the shared setup** (§25: a repository's, a connection's
   shared settings, a log source's, an agent's models) goes to the
   workspace too. Add it to the record in `crates/domain`, with a migration
   for its column (a connection's settings are one JSON column, checked in
   `connection.rs`), then `bun run gen:types`. Add it to what a change
   sends, `records` (`connection_settings` for a connection) and
   `patch_keys` in `config/shared.rs`, and to what the
   workspace sends back, `merge` in `workspace/mirror.rs`. What is one
   person's own (a token, the Jira email) stays out of all of them.

Never add a setting for `MAX_PANES` or `RESTORE_LIMIT`. The pane limit's
setting (PANE-2) raises `PANE_LIMIT`, and only as far as `MAX_PANES`.

## Add a backend event

Use an event when the backend knows something changed that the UI shows.
Don't make the UI ask on a timer.

1. **Emit it** from Rust: `let _ = app.emit("area:thing", payload);`, with
   a `Serialize` payload. Name it `area:thing`.
2. **Listen once**, in `Watchers` in `src/Watchers.tsx` for anything app-wide,
   or in the component that needs it, and unlisten on cleanup:

   ```ts
   useEffect(() => {
     const p = listen<Payload>("area:thing", (e) => { /* … */ });
     return () => { void p.then((un) => un()); };
   }, []);
   ```

   Usually the handler just calls a store refresh, which coalesces with
   any refresh in flight.
3. **Add a row** to the events table in `docs/architecture.md`. The guard
   checks that it is there, and that emit and listen agree.

## Add a view, tab or dialog

1. **One component per file** in `src/components/` (or `sidebar/`,
   `tickets/`). Past the default size ceiling (600 lines), split it.
2. **Read the store with selectors**, one field each:
   `const tasks = useStore((s) => s.tasks);`. Never `useStore()` whole.
   In callbacks, read with `useStore.getState()`, which does not subscribe.
3. **Fetch through `api.*`**, and through a store action if other views
   show the same data. Guard every async effect against a late answer:

   ```ts
   useEffect(() => {
     let current = true;
     api.thing(id).then((r) => { if (current) setThing(r); }).catch(fail);
     return () => { current = false; };
   }, [id]);
   ```

4. **Dialogs** are a `Modal` with `Field`s. The footer has `btn` Cancel and
   `btn btn-primary`. While `busy`, the buttons are disabled and `onClose`
   is blocked. Confirmations use `Confirm`, never `window.confirm`. A
   submit button that can be double-clicked needs a ref guard, not only
   state.
5. **Errors** of something the user just did show where they did it:
   `const failure = useFailure()`, `failure.fail(e)` in the `catch`,
   `failure.clear()` before trying again, and `<ErrorMessage>{failure.error}</ErrorMessage>`
   above a dialog's buttons or under the row of actions (NOTE-6). Only a
   failure with no place of its own, or one in the background, goes to the
   store's `fail(e)`: the top bar's status (WIN-2). For a multi-repo
   result, use `reportRepoResults`.
6. **Styles** go in `src/styles.css`, in the section for that area, using
   the studio's colour, type and radius names (`src/theme.css`, from the
   vault's `studio/design/tokens`). Follow the vault's `studio/design/principles.md` and
   `studio/design/components.md`: tone, not lines; one primary action per screen;
   chips tinted, never outlined. A colour Layer needs that the studio has
   no name for is a color-mix of those names, or a role in `theme.css` made
   from them (SET-5). Plain kebab-case classes. Reuse an existing class
   before adding a near-copy.
7. **A new top-level view** goes in `VIEWS` in `store.ts` and the top bar in
   `App.tsx`, and gets an entry in `docs/features.md`.
8. **Look at it** in the mock harness (`docs/testing.md`), including the
   empty scenario.

## Add an agent CLI

1. **Check that it can report its state**: hooks, a plugin, or its window
   title. PANE-1: a CLI that cannot say what it is doing is not offered.
   Write down how you confirmed it.
2. **Add an `AgentDef`** to `AGENTS` in `src-tauri/src/agents.rs`. Leave
   `resume_args` and `session_store` as `None`, unless resume is per folder
   and you have checked it.
3. **Pick or add an `Integration` variant.** A CLI with Claude-shaped hooks
   can reuse the Copilot pattern (`post_hook`, `HOOK_EVENTS`). A new
   variant needs arms in `prepare_launch` and `hook_activity` (the compiler
   insists). It also needs arms that the compiler will *not* ask for: in
   `title_reader`, if it reports through its title, and in the `matches!`
   in `commands/panes.rs` `plug_in`, if its config reads
   `VILLAIN_MCP_TOKEN`.
4. **Hand it the MCP server** without putting the token on its command
   line: a 0600 file, or an environment variable its config expands.
5. **Files it needs** go in the app's config folder. Anything written into
   a task folder goes in `GENERATED_FILES` (`commands/panes.rs`), so
   deleting the task removes it.
6. **Check `TRUST_MARKERS` and `LIMIT_MARKERS`** in `pty/screen.rs` against its
   wording.
7. **Tests**: copy `each_cli_is_handed_its_hooks_and_the_server_its_own_way`
   (`agents.rs`) and `a_hook_post_moves_its_pane_by_what_that_pane_runs`
   (`mcp.rs`).
8. **Docs**: add a row to the agent table in `docs/features.md` §3 (the
   guard checks it).
9. **Say what you ran.** Launch it in a task, watch its dot go working →
   asking → done, and call one MCP tool from it. If you could not, say so.

## Add an MCP tool

1. **Define it** in `tools()` in `src-tauri/src/mcp.rs`, with `tool(name,
   description, properties, required)`. The description is for the model,
   so say when to use it and what it returns.
2. **Handle it** in `call()`. Reuse the command's `_inner` function, inside
   `commands::blocking` if it does git or disk work. The handler is on the
   async runtime.
3. **Anything that changes shared state irreversibly** goes in `DANGER` and
   takes `confirm`. Reads never do.
4. **Anything that reads another pane's output** checks `agents_read_panes`.
5. **Add a row** to the tool table in `docs/features.md` §12 (the guard
   checks it). If agents should use it unprompted, add it to the tools that
   `write_chat_context` (`commands/panes.rs`) names.
6. **Tests**: the existing `tools_list_names_the_danger_tier` and
   `danger_tools_advertise_confirm` cover a new danger tool once it is in
   `DANGER`.

## Add a Jira, GitHub or Slack call

1. **Put the HTTP in `src-tauri/src/integrations/<service>.rs`** as a method
   on the client, and the orchestration in `commands/<service>.rs`.
2. **Anything from outside that goes into a path** goes through `segment()`
   (Jira) or `path_segment()` (GitHub) first. Text going into JQL goes
   through `jql_string`.
3. **Page it.** Keep the cap explicit, and return whether it was hit.
4. **Ask the site** instead of assuming names: status categories, hierarchy
   levels, `createmeta`, the field's schema.
5. **In a multi-repo command**, a failed call becomes that row's `error`,
   not the whole command's error and not an empty result.
6. **Secrets** never go in an error message. Watch for URLs that carry
   one: reqwest's errors end with the URL they were for.
7. **Test the parsing** against a JSON body copied from the real API (with
   names made up). See `integrations/github.rs` tests.

## Add a git operation

1. **A named function in `src-tauri/src/git.rs`** (or `git/store.rs`, for
   the app's copies, `git/upkeep.rs` for Sync and Clean up). `run` is
   private, so there is no other way. A
   repo-level operation runs in `commands::repo_for` (new worktrees) or
   `commands::owner_of` (an existing one), never in `Project.path`.
2. **Spell out what you rely on** (see the table in
   `docs/architecture.md`, "Git"). The user's config applies: `--no-ext-diff`
   for any diff, `--untracked-files=normal` for status, `-z` for paths.
3. **Validate anything from outside** that becomes an argument
   (`check_names`, `commit_id`), and put `--` before paths.
4. **Test it with real git** in a temp dir. `fixture()` is a one-commit
   repo; `remote_and_worktree()` is a bare remote with a clone and a
   worktree. The tests run under your global git config, so write them to
   pass whatever it says.
5. **Call it from a command through `blocking`.**

## Add a spec toolset

A toolset (§19) is data: a manifest and a paragraph. A team adds one of its
own in Settings → Code, as JSON, with no release (SPEC-27). Ship one with
Layer only when everyone should have it.

1. **Write the manifest**, `src-tauri/toolsets/<id>.json`: `id`, `name`, an
   optional pinned `version`, `files` and `steps` (globs relative to the task's
   `spec/`: `*` within a segment, `**` across segments; every `steps` glob lies
   inside `files`), `install` (steps, in order: `{"run": [argv…], "cwd":
   "spec"|"task", "timeout_secs": n}`, `{"fetch": url, "ref": ref}`,
   `{"skills": glob}` for skill folders in the fetched copy; `{version}`,
   `{task}`, `{openspec_tools}` and `{skill_dirs}` are filled in), and
   `commit_to`. A step names exactly one kind and `run` is an argv, never a
   shell string. Pin the version of anything it fetches or runs.
2. **Write the paragraph**, `src-tauri/toolsets/<id>.md`: what agents are told
   about the toolset, in the context file's Spec section (SPEC-10). Write
   `{spec_dir}` where the working copy's absolute path goes (and `{version}`
   where its pinned version goes), and say where its files are saved, by that
   path, so a skill or tool never writes into a repository's worktree. A
   command it runs must be one agents can run: say how to run it if it is not
   on the PATH. Say how its steps are grouped for several repositories (a
   first-level `# <repo>` heading or `[repo]` tags, SPEC-34).
3. **List it** in `builtin()` in `src-tauri/src/spec/toolset.rs`, a pair of
   `include_str!`. The order there is the order of every pick list. Settings,
   Start work and the Repos view read the list; nothing else names it.
4. **Mirror a repository's root.** Commit spec puts a file at the same path in a
   repository (SPEC-35), which is what a toolset wants. Only Kiro's flat files go
   elsewhere, by its id (`spec_commit.rs`, `target_path`; `spec_work.rs`,
   `pr_section`). Do not add a toolset that needs another place without
   changing that deliberately, and `commit_to` alone does not move a file.
5. **Give it an entry file**, if it has one: the file that says what the work
   is, whose first 8 KB the context file quotes. Add its id and glob to
   `entry_file` in `src-tauri/src/commands/task_context.rs`; without one the
   section has the paragraph only.
6. **Tests**, at the bottom of the file they test, real folders in a temp dir:
   - `spec/toolset.rs`: add the id to `every_builtin_toolset_is_valid`, and its
     pinned values to `the_builtin_manifests_carry_the_pinned_values`. `validate`
     refuses: an id that is not lowercase letters, digits, `-` or `_`; no name;
     a version that is not one path segment; no `files`; a glob with `..`, an
     absolute path or an empty segment; a `steps` glob outside `files`; a `run`
     with no command, a `fetch` with no URL or ref, a `skills` glob that leaves
     the fetched copy; a `commit_to` that leaves the repository. Mutate one
     field of a valid manifest per test.
   - `commands/spec_install.rs`: an install test with the fake `Runner` that
     file has (`installing_openspec_runs_its_pinned_init_in_the_spec_folder`,
     `installing_superpowers_copies_its_skills_into_every_skill_folder`), so no
     `npx` runs and no network is touched.
   - `spec/owned.rs`: a step file of its shape, which repository owns each step.
7. **Mock harness**: a world for it in `src/__mock__/spec-worlds.ts` and a
   `toolset` value in `docs/testing.md`, so the tab is clicked through with its
   files and steps.
8. **Docs**: list it in SPEC-26 in `docs/features.md` §19, with its files, steps
   and install, and change any requirement the toolset makes untrue.
9. **Say what you ran.** Install it into a task and read what it wrote in the
   task folder (`.spec-install.json` lists it), start an agent and ask it for
   its skills (an agent CLI Layer does not give the task folder to will not see
   them from a worktree, SPEC-32), and tick a step from the Spec tab, the file and
   `spec_task`. If you could not, say so.

## Fix a bug

1. **Reproduce it**, in a test if it is backend logic, or in the mock
   harness if it is UI.
2. **Write the test that fails**, named as the sentence that should be true
   (`a_second_rebase_before_pushing_is_not_refused_by_the_first`).
3. **Fix it.** Leave a comment at the fix that says what went wrong before,
   and why the code is now shaped to prevent it. That comment is often the
   only record of the bug.
4. **Check the requirement** in `docs/features.md`. If the bug was a missing
   requirement, add one. If it is listed as a known gap, remove it.
5. **Look for the same bug elsewhere.** The fixes in this codebase usually
   found a second and third copy.

## Splitting a file

When a file hits its ceiling (`file-size`), split along a seam that already
exists:

- **A Rust command module**: move one feature's commands and their helpers
  into a new `commands/<area>_<feature>.rs`, and `pub use` it from
  `commands/mod.rs`. Keep `_inner` functions with their command.
- **A component**: move a dialog or a sub-panel into its own file beside
  it, passing props rather than reading the parent's state. Shared hooks
  (`useRequiredFields`) go in their own file.
- **`store.ts`**: move pure derived helpers (`paneState`, `needsYou`,
  `groupByEpic`) to `src/lib/`, and keep state and actions in the store.

Move code without changing it in the same commit, so the diff of the move
reads as a move. Raising a ceiling in `scripts/guard.ts` instead is allowed
when the growth really belongs there. Say why in the PR.
