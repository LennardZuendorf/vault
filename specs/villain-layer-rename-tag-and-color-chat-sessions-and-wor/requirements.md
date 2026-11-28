# Requirements

Updated: 2026-11-27

## Goal

Make long-lived chats and work tasks easier to recognize in Layer without changing the work or conversation behind them.

## Requirements

- R-1: WHEN a user opens an open or waiting chat's row menu in Chat, or an existing Layer task's row menu in Work, THE SYSTEM SHALL offer a keyboard- and pointer-accessible **Edit details…** action to set, change, or clear a local display name, tags, and a color independently. This includes Slack chats as shown in Layer and stored tasks such as started subtasks and Layer epics. Jira-only epic headings and unfiled spec steps are not tasks for this purpose.
- R-2: WHEN a chat has a custom display name THE SYSTEM SHALL prefer it over the automatic title in the Chat list, All agents, desktop agent pickers, and Work's link to the chat. Clearing it shall restore the latest automatic title or the existing fallback. A CLI title update shall not replace a custom display name.
- R-3: WHEN a work task has a custom display name THE SYSTEM SHALL show it in Work's task rows and selected-item heading without hiding the existing ticket, branch, or repository identifiers. Clearing it shall restore the name that surface would have shown before the edit.
- R-4: WHEN an item has tags or a color THE SYSTEM SHALL show that detail in its Chat or Work rows. Tags shall be readable text. Color shall be optional, chosen from six named palette options or **No color**, and shown separately from agent, checkout, and review status. Items without details shall retain their existing appearance.
- R-5: WHEN an edit is saved successfully THE SYSTEM SHALL display it without a restart and retain it in the current desktop profile after relaunch. Chat details shall follow the room through restart, resume, waiting, reopening, and Slack idle stop or Slack reopening. Closing an ordinary chat or explicitly forgetting a waiting chat shall remove its details. A task shall keep its details as long as its task record exists and lose them when that record is removed.
- R-6: WHEN the user searches Work THE SYSTEM SHALL match custom task names and tags as well as all existing fields, with the same case-insensitive, all-words matching and epic behavior. Editing details shall not switch the selected grouping or change the default stage ordering, pins, or Slack thread links.
- R-7: WHEN details are saved THE SYSTEM SHALL trim surrounding whitespace, treat a blank name as no override, and ignore empty tags and case-insensitive duplicate tags, preserving assignment order and reusing the first saved catalog spelling. It shall reject multiline or control-character text, names longer than 120 Unicode characters, tags longer than 40 Unicode characters, more than 12 distinct tags per item, and colors outside the palette.
- R-8: WHEN the user cancels, or the item disappears or the save fails, THE SYSTEM SHALL apply none of the draft changes and shall not recreate a missing item. On failure it shall keep the editor open, preserve the draft, and show an inline error.
- R-9: WHEN a profile has no saved details or was created before this feature THE SYSTEM SHALL load it with no overrides, tags, or assigned colors. Editing details shall not change item identity, worktrees, branches, ticket or PR data, agent prompts or context, session or transcript history, or workspace sync.
- R-10: WHEN a user opens Settings → Tags THE SYSTEM SHALL let them view and add reusable text tags, including tags not yet assigned to an item. Chat and Work shall offer tags from the same catalog within the current desktop profile. Adding a tag while editing an item shall make it reusable in the same profile. Removing a tag from an item shall not remove it from the catalog.
- R-11: WHEN an authenticated Layer agent calls `task_rename`, `task_set_tag`, or `task_set_color` THE SYSTEM SHALL let it rename or reset, add or remove one text tag, or set or clear the color, respectively. The target shall be an explicit work task in the current profile, or the caller's work task or standalone Chat session if no task ID is supplied. Each operation shall use the desktop editor's validation, preserve other fields, and update visible details without a restart. It shall not change Git, Jira, GitHub, or Slack state.
- R-12: WHEN the user selects **By tag** in Work THE SYSTEM SHALL group stored Work items under every assigned tag and show items without tags under **No tag** at the end. The tag view replaces the epic tree while selected, so started subtasks and Layer epics can be found by tag too. Each group retains the existing stage order. The selection survives relaunch, works without a tracker, and leaves the other grouping modes unchanged.

## Out of scope

- Renaming an upstream issue, PR, Slack thread, Git branch, room folder, or CLI conversation.
- Chat search or grouping, bulk editing, and custom color values.
- Tag colors, catalog rename/delete, and sharing tags across profiles, devices, or teammates.
- Phone and web editing, and agent-facing controls outside R-11. The Henchman, routine runs, and an epic's embedded chat are not standalone Chat sessions.
- Importing conversations or changing how chats and tasks are created, resumed, finished, or deleted.

## Open questions

- Q-1: What does a global tag catalog mean in Layer?
  - One catalog per desktop profile, shared between Chat and Work.
  - One catalog across all Layer profiles on this Mac, with no sync.
  - One catalog per workspace, synced across Macs and available to its members.
  Answer: One catalog per desktop profile, shared between Chat and Work.
