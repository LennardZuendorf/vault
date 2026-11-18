# Layer image drop implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drop images into a Layer conversation to save local copies and insert their paths into the draft without sending it.

**Architecture:** The desktop message box receives Finder files through the webview's HTML drag events. A Tauri command resolves the destination from the actual pane, validates each file, saves it, and returns its absolute path. The composer appends successful paths to the current draft and still sends through `acp_prompt` only on explicit submission.

**Tech Stack:** React 19, Tauri 2, Rust, and Bun.

**Spec:** [Layer image drop design](../specs/2026-11-18-layer-image-drop-design.md). Layer's live requirements are in `code/apps/layer/docs/features.md`.

## Global constraints

- PNG, JPEG, GIF and WebP files, up to 10 MiB each and 10 files per drop.
- Preserve originals and existing attachments. Keep copies on disk when references are removed.
- No writes into Git checkouts. No writes through an `attachments/` symlink.
- Conversation panes only. No automatic send or broadcast. No new dependencies.
- Paths below are relative to `code/apps/layer/`. Run commands from that folder. Do not commit without an explicit request.

## Review focus

- A task agent launched inside a checkout still uses the task's `attachments/` folder (task 1).
- A duplicate name, symlink, or checkout named `attachments` cannot overwrite or redirect a save (task 1).
- A non-image named `.png` does not produce a file; a valid image with no MIME type still does (tasks 1–2).
- Typing or pressing Enter during a slow save does not lose text or send a partial draft (task 2).
- A mixed drop reports what failed and retains successful paths; a file dropped anywhere else cannot navigate the window (task 2).

---

### Task 1: Save a dropped image to the pane's workspace

**Files:** Modify `docs/features.md`, `src-tauri/src/commands/mod.rs`, and `src-tauri/src/lib.rs`. Create `src-tauri/src/commands/attachments.rs`, with tests at the bottom.

**Produces:** `save_chat_image(app: AppHandle, pane_id: String, name: String, data: String) -> Result<String>` (async Tauri command; `data` is standard base64, return value is the saved absolute path). Use a testable `save_chat_image_inner(state: &AppState, pane_id: &str, name: &str, data: &str) -> Result<String>`.

- [ ] **Record the contract first.** Add `ACP-20` after `ACP-19` in `docs/features.md`. Keep `ACP-16` (paste) unchanged.
- [ ] **Write failing Rust tests.** Test a repo-rooted pane resolving to `<task>/attachments/`, and two standalone chats resolving to their own `<room>/attachments/`. Saving a PNG named `Screen Shot.png` twice must preserve both copies (`Screen Shot.png`, `Screen Shot 2.png`) and their bytes. Traversal names (`../shot.png`), control characters, a fake PNG, oversized data, a missing or stopped pane, both checkout collisions, and a symlinked attachment folder must produce no new file. An existing symlink at the filename must not be followed or overwritten.
- [ ] **Confirm red.** Run `bun run build`, then `cargo test -p villain-layer --lib commands::attachments::tests`. Confirm that the new tests fail because saving is not implemented, not because of unrelated setup.
- [ ] **Implement the command.** Use `commands::blocking` for decoding and disk I/O. Resolve `PaneInfo` with `state.ptys.info`, require a running conversation, use `Task.root` for task panes, and for `CHAT_TASK_ID` use the pane's verified room under the workspace's `_chat` directory. Canonicalize roots and check against the registered checkout paths before writing. Refuse a symlink or non-directory at `attachments/`; open the directory without following symlinks and create each file exclusively relative to it. Accept a safe filename component only, derive its extension from `integrations::github::kind_of`, and bound base64 length before decoding and bytes after decoding. Allocate numbered suffixes without replacing files. Remove only the file created by this attempt if writing it fails. Register `commands::save_chat_image` in `generate_handler!`.
- [ ] **Confirm green.** Re-run the targeted Cargo tests. Expect all attachment tests to pass.

### Task 2: Route a drop to the conversation draft

**Files:** Modify `src/components/AcpPane.tsx`, `src/styles-acp.css`, `src/lib/api.ts`, `src-tauri/tauri.conf.json`, `src/main.tsx`, `src/__mock__/boot.ts`, and `src/__mock__/acp.ts`. Create `src/lib/chatDrop.ts` and `scripts/chatDrop.test.ts`.

**Consumes:** `save_chat_image(paneId, name, data)`, returning the saved absolute path.
**Produces:** `api.saveChatImage(paneId: string, name: string, file: File): Promise<string>`; `appendAttachmentPaths(draft: string, paths: readonly string[]): string`; a shared `preventFileNavigation(event: DragEvent): void` registered at app boot.

- [ ] **Write failing frontend tests.** Assert that an empty draft becomes `"/task/attachments/Screen Shot.png"`, that a nonempty draft keeps its exact text followed by the quoted paths on separate lines, and that an empty path list changes nothing. In the mock, exercise a delayed save, typing into the same draft, a mixed success/failure drop, and a file drop outside the box. Assert no `acp_prompt` before explicit Send, the correct `paneId` on save, and no `save_chat_image` call outside the box. Inject a valid PNG with `type: ""`; it must still be submitted for backend validation.
- [ ] **Confirm red.** Run `bun test scripts/chatDrop.test.ts` and replay the mock regression case at `/mock.html?task=t-login&tab=terminals` with `pane-acp` selected. Expect the new tests to fail before the drop flow exists.
- [ ] **Implement.** Give the main window's HTML handlers control with `dragDropEnabled: false`, as Postrider does. Install the same file-only navigation guard in both `src/main.tsx` and the mock entrypoint. The active composer accepts file drags, shows a subtle token-backed highlight, and reads at most 10 `File` objects in order. Report additional files and those over 10 MiB inline. Send each accepted-size file through `api.ts` as `{ paneId, name, data }`; strip the data URL prefix, but do not filter by MIME. Use a synchronous in-flight guard so neither Enter nor Send can submit while saving, even before React updates. If another drop arrives in the same pane while saving, reject it inline with “Still saving images. Drop them again when it finishes.” Append successful paths to the latest draft with a functional update, using `JSON.stringify` for each path. Report failures under the composer with `useFailure` and retain successful paths. Leave paste and terminal handlers untouched. Add a deterministic `save_chat_image` mock response.
- [ ] **Confirm green in the browser.** Re-run the Bun test. In the mock, drop multiple files including one rejected file, type while a save is pending, and inspect `__mock.calls`. Verify that only the target pane changes, the draft remains editable, paths appear in order, and explicit Send is the first `acp_prompt`. Verify an ordinary text drop and paste still work.
- [ ] **Verify the desktop.** Run `bun run db` and `bun run check`. In a separate dev or packaged Layer build, drag real Finder files into a task conversation and a standalone chat, including one image near the 10 MiB limit. Verify copies and paths on disk, no Git changes, no automatic send, and no navigation or insertion on a drop outside the composer. Confirm a real task agent can open a saved image from the submitted path. Report separately if the Finder or real-agent checks cannot run.
