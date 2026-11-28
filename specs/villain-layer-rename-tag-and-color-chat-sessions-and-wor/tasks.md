# Implementation Plan: Chat and Work details

Updated: 2026-11-28

**Spec:** [Requirements](requirements.md) · [Design and contracts](design.md)

**Goal:** Give chats and work tasks durable local names, reusable text tags, and colors, and group Work by tag.

**Architecture:** Layer's Rust desktop backend owns item details and the profile's tag catalog. The React frontend and three authenticated MCP tools use the same backend edits. No server schema or dependencies change.

Begin only after approval. For each implementation step, add the indicated tests first, observe them fail, implement the smallest change, and run them again. Tick each step when its checks pass, before starting the next one.

File paths refer to `vault/code/apps/layer/`. `commands/` is under `src-tauri/src/`; unqualified component names are under `src/components/`.

## Tasks

- [x] 1. Update Layer's behavior contract in `docs/features.md`: cover Chat, Work, Settings, and the three MCP tools. Amend PANE-12, TASK-17, and TASK-19. Document IPC, the event, and profile-local storage in `docs/architecture.md`. Check that title sources and item identities do not change (R-1–R-12, C-1–C-4).
- [x] 2. Define and validate `ItemDetails` in `src-tauri/src/config/item_details.rs`. Test blank and trimmed values, duplicate case variants, ordering, exact Unicode length and tag-count boundaries, control characters, and out-of-palette input. Run `cargo test -p villain-layer --lib item_details` (R-7, R-9, C-1).
- [x] 3. Add the profile maps, text-tag catalog, and persist-before-publish edit in `src-tauri/src/config.rs` and its details module. Add field-level edit and single-tag assignment helpers in `commands/item_details.rs`. Test old-config defaults, reload, profile isolation, first-spelling reuse, concurrent distinct-field changes, clear, missing target, and a refused disk write leaving both item and catalog unchanged. Run `cargo test -p villain-layer --lib item_details` (R-5, R-7–R-11, C-1–C-4).
- [x] 4. Tie detail cleanup to ordinary chat close, waiting-chat forget, and task removal in `commands/panes.rs`, `commands/panes_restore.rs`, `commands/tasks.rs`, and `commands/projects.rs`. Cover ordinary versus Slack close, Slack waiting/explicit forget, fresh pane ID with the same room, and a task dropped when its last repository is removed. Run the Layer Rust library tests (R-5, R-9, C-1, C-2).
- [x] 5. Wire the desktop list projections, item writes, and catalog commands in `commands/`, `src-tauri/src/lib.rs`, `src/lib/api.ts`, and `src/lib/types.ts`. Wire the change event in `src/Watchers.tsx` and refresh handling in `src/store.ts`. Implement the same contract in `src/__mock__/world.ts` and `src/__mock__/boot.ts`. Test that stale refreshes cannot overwrite saved edits and existing MCP lists keep their serialization. Run the Layer Rust library tests and `bun test scripts` (R-5, R-8–R-11, C-1–C-3).
- [x] 6. Add the three requested tools through `src-tauri/src/mcp.rs` and a focused details tool module. Test explicit Work targets, caller Work and Chat targets, ineligible and missing callers, unknown task IDs, validation, clears, idempotent tag assignment/removal, and notification after successful durable edits. Run the Layer Rust library tests (R-5, R-7–R-11, C-1, C-3, C-4).
- [x] 7. Add `TagsSettings.tsx` and the tag catalog tab in `Settings.tsx`. Add the shared details editor and row indicators in `ItemDetails.tsx` with existing UI primitives and palette tokens. Verify Settings can add an unused tag, both editors can reuse it, and a case variant does not add another entry (R-1, R-4, R-7, R-8, R-10, C-1–C-3).
- [x] 8. Wire open and waiting Chat rows in `ChatView.tsx`, `ChatList.tsx`, and `WaitingPanes.tsx`. Add display-name precedence in `src/lib/derive.ts`, `ChatLink.tsx`, and `AgentsView.tsx`. Run `bun test scripts/pane-name.test.ts scripts/pinned-chats.test.ts scripts/slack-chats.test.ts` and check open, pinned, waiting and Slack rows in the mock (R-1, R-2, R-4, R-5, R-8, C-1–C-3).
- [x] 9. Wire Work rows in `sidebar/Sidebar.tsx`, `sidebar/TaskRow.tsx`, and `sidebar/EpicRow.tsx`; apply the name in `src/App.tsx` and `EpicView.tsx`. Extend `src/lib/task-search.ts` to match aliases and tags. Add **By tag** via `src/lib/grouping.ts` and the existing grouping control. Test multi-tag membership, **No tag** at the end, stage order, a user tag named `No tag`, and search over nested tasks. Run `bun test scripts` and check the grouping survives reload in the mock (R-1, R-3–R-6, R-8, R-12, C-1–C-3).
- [x] 10. Run `bun run check` from `vault/code/apps/layer` with Postgres for the server tests. In `/mock.html`, walk Chat, Work, and Settings with pointer and keyboard, exercise grouping, clear, cancel, and failed saves, and check existing status indicators at narrow widths in day/night. Verify relaunch and the MCP flows in Layer Dev or report what could only be tested through fixtures. Record evidence before requesting ship approval (R-1–R-12, C-1–C-4).
