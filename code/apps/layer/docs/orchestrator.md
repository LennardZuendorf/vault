# Henchman, the orchestrator (draft)

A design draft, not a requirement yet. Requirements go into
`features.md` once the story below is agreed. Its name: **Henchman**.

## The problem

Twenty tickets are open in Layer, and there is no longer one place that
says how things stand. Today that means:

- Each task has to be opened to see where it is: an agent waiting, red
  checks, review comments, a spec not approved, nothing happening for
  two days.
- "N need you" counts agents, not work. A merged task that's waiting to
  be finished, a review someone asked you for, and a ticket nobody has
  started all count as nothing.
- When an agent finishes a turn, nobody pushes it to the next step (open
  the PR, fix CI, answer the review). You do it, task by task. That's
  the outer loop that LOOP left open.
- An epic made in Layer has a chat that keeps its sub-tasks moving
  (EPIC-11). Everything else, including tasks under Jira epics and tasks
  with no epic, has nothing like it.

## The idea

The epic's chat, one level up: **one standing conversation over all of
Layer**, in a tab of its own, first in the top bar: the one who keeps
the lair running.

The levels:

```
Henchman            all of Layer: what needs you, what's next, what's stuck
  └ epic chats      one epic's sub-tasks (EPIC-11)
      └ task agents the code, in a task's worktrees
```

It has three jobs:

1. **Brief.** Where you stand, in a few lines: what needs you (ranked),
   what changed since you last looked, what's stuck, what can be
   finished. Everything that's fine is one line ("14 others: nothing
   new").
2. **Act.** On your word: tell an agent something, start the next
   ticket or sub-task, put an agent on its loop, open PRs, finish merged
   tasks. Anything that changes shared state asks first, as MCP-3 already
   requires.
3. **Watch.** The app tells it what happens as it happens (an agent asks
   or finishes, checks go red, review comments arrive, a PR merges). It
   handles what its powers allow and brings the rest to you. This is the
   outer loop.

What it never does: write code, edit files, merge, or answer anyone in
Jira, GitHub or Slack on your behalf without asking. It runs in a room
of its own (as a chat does, CHAT-1), not in a folder that holds
worktrees, so it doesn't have the epic chat's known gap.

## One morning

1. **9:00.** You open Layer on the Henchman's tab. It shows its brief,
   not a wall of twenty rows:
   > **3 need you.** ACME-12's agent asks which endpoint to keep. Ana
   > asked for changes on ACME-40. Your review is asked on ACME-55.
   > **Overnight.** ACME-31 merged: finish it? ACME-33's checks went
   > red: I sent the failure to its agent, and it's fixing them.
   > **14 others:** nothing new.

   Each key opens what it names, by the usual route (NOTE-4).
2. You type "12: keep v2. Finish 31." It tells ACME-12's agent, and
   shows a card to confirm Finish for ACME-31 (worktrees go, the ticket
   moves). One click.
3. "What's next?" It answers: "Of 6 tickets not started, ACME-61 is
   ready (spec approved, waits on nothing). ACME-200 has 2 sub-tasks
   ready." You say "start 61 and 200's next". It shows one confirm card,
   and both start.
4. During the day you work inside one task. The Henchman keeps watching.
   ACME-40's agent pushes fixes for Ana's review, and the Henchman says:
   "ACME-40 answered Ana's comments. Ask her to review again?" with a
   button.
5. **18:00.** You're away. On Slack you ask @DrDoom "status?" and get
   the same brief in the thread, from the same Henchman.

## What it's built from

Most of it already exists:

| Already there | Used for |
|---|---|
| The epic's chat: ACP, its column, its context file (EPIC-11) | the conversation and its column |
| `epic_subtasks`, `subtask_start`, `subtask_tell` (EPIC-12) | the same three tools, for every task rather than one epic's sub-tasks |
| `list_tasks`, `list_panes`, `task_prs`, `start_work`, `open_prs`, `finish_task` | reading and acting, as they are |
| `attention.rs`, `news.rs`, the PR sweep | the events it is told |
| Loops (§21) | what "put it on its loop" starts |
| The Slack bot (§24) | reaching it while you're away |

New:

- `work_overview`: one call for every task: its ticket and status, stage,
  epic, agents and their states, loop, PRs (checks and reviews), spec
  steps ticked, last activity, and what it waits for.
- `task_tell` and `task_start`: `subtask_tell` and `subtask_start`
  widened to any task.
- Events sent to it while it's idle, batched into one message, never one
  per event.

## Steps, one at a time

1. **Brief.** The Henchman's tab with the lair beside it,
   `work_overview`, and a brief when it opens or when you ask. Reading
   only, plus the tools that exist today.
2. **Act.** `task_tell`, `task_start`, starting a loop, with confirm
   cards.
3. **Watch.** Events pushed to it, within the powers you give it.
4. **Away.** @DrDoom and the phone reach the same Henchman.

Each step is used for a few days before the next one starts.

## The screen

A tab of its own, first in the top bar: **Henchman**, its count the tasks
that need you. Work, with its sidebar and All agents, stays as it is.

- **Left: the conversation**, in a centred column: a title ("The lair at
  9:00") and a line of counts (20 open · 3 need you · 1 to finish · 5
  moving · 2 not started · 9 quiet), then the brief, confirm cards,
  suggestions and what you tell it. Its box at the bottom.
- **Right: the lair**, a narrow column that stays current without being
  asked: Needs you and To finish as rows (key, title, what it waits for)
  with their one button (Open, Finish); Moving, Not started and Quiet
  folded to counts, opened with a click. Chats aren't tasks and stay in
  Chat.

## Decided (Nov 15)

- **Where:** its own tab, first in the top bar. A first sketch put it
  beside the Work overview, and was dropped.
- **Layout:** the conversation, with the lair beside it (B of two
  sketched; A put everything in the conversation).
- **On its own:** it only passes red checks and review comments to the
  task's own agent. Starting work, finishing, answering agents' questions,
  and anything in Jira, GitHub or Slack wait for your click.
- **In flight:** it suggests which tasks to leave alone for now and why;
  it stops and hides nothing.
- **Name:** Henchman.
