# Henchman, the orchestrator (draft)

A design draft, not a requirement yet. Requirements go into
`features.md` with the code of each step below. Its name: **Henchman**.

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
- Chats, epic chats and task agents each have to be opened to be asked
  anything, or told anything.
- An epic made in Layer has a chat that keeps its sub-tasks moving
  (EPIC-11). Everything else, including tasks under Jira epics and tasks
  with no epic, has nothing like it.

## The idea

The epic's chat, one level up: **one standing conversation over all of
Layer**, in a tab of its own, first in the top bar. The Henchman keeps
the lair running and helps you get tickets done.

The levels:

```
Henchman            all of Layer: what needs you, what's next, what's stuck
  ├ chats           the Chat tab's conversations (§5)
  └ epic chats      one epic's sub-tasks (EPIC-11)
      └ task agents the code, in a task's worktrees
```

It has four jobs:

1. **Brief.** Where you stand, in a few lines: what needs you (ranked),
   what changed since you last looked, what's stuck, what can be
   finished. Everything that's fine is one line ("15 others: nothing
   for you").
2. **Talk.** With every agent and chat open in Layer: task agents, epic
   chats, and the Chat tab's chats. It reads what they last said, passes
   your words on, asks them things to answer you ("what did the staging
   chat find?"), and starts a chat on your word.
3. **Act.** On your word: start a ticket or sub-task, put an agent on its
   loop, open PRs, ask for a review again, finish merged tasks. Anything
   that changes shared state asks first, as MCP-3 already requires.
4. **Drive.** Hand it a ticket ("get ACME-181 done") and it carries it to
   merged: it starts it, keeps its agent moving between turns, passes on
   red checks and review comments, and brings you only the points that
   need you, each as a card (open the PR, ask for review again, finish).
   This is the outer loop, one ticket at a time.

The app tells it what happens as it happens (an agent asks or finishes,
checks go red, review comments arrive, a PR merges), batched while it's
busy. It handles what its powers allow and brings the rest to you.

What it never does: write code, edit files, merge, or answer anyone in
Jira, GitHub or Slack on your behalf without asking. It runs in a room
of its own (as a chat does, CHAT-1), not in a folder that holds
worktrees, so it doesn't have the epic chat's known gap.

## One morning

1. **9:00.** You open Layer on the Henchman's tab. It shows its brief,
   not a wall of twenty rows:
   > **3 need you.** ACME-123's agent asks to write two files. Ana asked
   > for changes on ACME-161, and its agent asks where the new test goes.
   > ACME-130's agent finished: your turn.
   > **Overnight.** ACME-160 merged. ACME-172's checks went red: I sent
   > the failure to its agent, and it's fixing them.

   Each key opens what it names, by the usual route (NOTE-4).
2. You type "161: put the test in session.test.ts. Finish 160." It tells
   ACME-161's agent, and shows a card to confirm Finish for ACME-160
   (worktrees go, the ticket moves). One click.
3. "What did the staging chat find?" It reads "Why is staging slow" in
   Chat: "The search index rebuilds every five minutes. It suggests a
   ticket." You say "tell it to draft one", and it passes that on.
4. "What's next?" It answers: "Of 2 tickets not started, ACME-181 is
   ready: nothing waits on it." You say "get 181 done". One card: start
   ACME-181 in web with Claude Code. You click, and the Henchman drives
   it:
   - The agent ends a turn with spec steps left: it tells it to go on.
   - 14:10, steps ticked and checks green: a card to open the PR.
   - 16:00, Ana's review comments: passed to the agent, no card.
   - Fixes pushed: a card to ask Ana to review again.
   - Next morning, merged: a card to finish it.
5. During the day you work inside one task. The lair keeps itself
   current, and the Henchman speaks only when something needs you.
6. **18:00.** You're away. On Slack you ask @DrDoom "status?" and get
   the same brief in the thread, from the same Henchman.

## What it's built from

Most of it already exists:

| Already there | Used for |
|---|---|
| The epic's chat: ACP, its context file (EPIC-11) | the conversation |
| `epic_subtasks`, `subtask_start`, `subtask_tell` (EPIC-12) | the same tools, for every task, chat and agent |
| `list_tasks`, `list_panes`, `pane_output`, `task_prs`, `start_work`, `open_prs`, `finish_task` | reading and acting, as they are |
| `attention.rs`, `news.rs`, the PR sweep | the events it is told |
| Loops (§21) | what "put it on its loop" starts |
| The Slack bot (§24) | reaching it while you're away |

New:

- `work_overview`: one call for every task: its ticket and status, stage,
  epic, agents and their states, loop, PRs (checks and reviews), spec
  steps ticked, last activity, what it waits for, and whether it's
  driven; then the open chats and what each last said.
- `pane_read`: what any agent or chat last said: its last turn over ACP,
  or a terminal's recent output under "Let agents read terminal output"
  (MCP-4), as `pane_output` reads it.
- `pane_tell`: one message to any running agent or chat, typed into a
  terminal or sent as a prompt over ACP, as `subtask_tell` sends it.
- `chat_start`: a new chat in Chat, with its first message.
- `task_start`: `subtask_start` widened to any ticket.
- `drive` and `stop_driving`: which tickets it carries, shown in the lair.
- Events sent to it, batched into one message while it's busy, never one
  per event.

## Steps, one at a time

1. **Brief and talk.** The tab and the lair, `work_overview`, a brief
   when it opens or when you ask, and talking to any agent or chat
   (`pane_read`, `pane_tell`, `chat_start`). Nothing in this step changes
   shared state.
2. **Act.** `task_start`, loops, opening PRs, asking for review again,
   finishing, each with its card.
3. **Drive.** Told when any agent ends a turn, it answers what you've
   answered before and says "go on" to an agent with planned steps left,
   for every task and chat; then `drive`, red checks and review comments.
   Standing orders, kept in the lair, are what it answers from.
4. **Away.** @DrDoom and the phone reach the same Henchman.

Each step is used for a few days before the next one starts.

## The screen

A tab of its own, first in the top bar: **Henchman**, its count the tasks
and chats that need you. Work, with its sidebar and All agents, stays as
it is.

- **Left: the conversation**, in a centred column: a title ("The lair at
  9:00") and a line of counts (20 open · 3 need you · 1 to finish · 5
  moving · 2 not started · 9 quiet), then the brief, confirm cards,
  suggestions and what you tell it. Its box at the bottom.
- **Right: the lair**, a narrow column that stays current without being
  asked: Needs you and To finish as rows (key, title, what it waits for)
  with their one button (Open, Finish); Moving, Not started, Quiet and
  Chats folded to counts, opened with a click. A chat that asks you is
  in Needs you. A ticket it drives is marked so.

## Decided (Nov 15)

- **Where:** its own tab, first in the top bar. A first sketch put it
  beside the Work overview, and was dropped.
- **Layout:** the conversation, with the lair beside it (B of two
  sketched; A put everything in the conversation).
- **Talk:** it talks with every open agent and chat, not only tasks'.
  What you tell it to pass on goes straight away; it starts a chat on
  your word.
- **On its own:** it passes red checks and review comments to the task's
  own agent. Starting work, opening PRs, asking for review, finishing,
  and anything in Jira, GitHub or Slack wait for your click, unless a
  standing order says otherwise (below).
- **In flight:** it suggests which tasks to leave alone for now and why;
  it stops and hides nothing.
- **Name:** Henchman.

## Decided later (Nov 15): it drives

Step 1 only listed, and the agents kept asking the same questions. So:

- **Answers on its own:** when an agent or chat ends a turn, the Henchman
  hears of it. It answers a question you've answered before, from its
  standing orders, and says "go on" to an agent that stopped with planned
  steps left. A new question comes to you. Each answer it gives is one
  line in its conversation.
- **Standing orders:** what you tell it to answer from now on, kept
  across restarts, listed in the lair, each removable. You add one by
  telling it ("always say yes to running the tests").
- **Outward actions:** a standing order may let it say yes to marking a
  PR ready, posting in Slack or commenting in Jira, once you've said so.
  Nothing outward happens without such an order or your click.
- **Which:** every task agent and chat, except one you tell it to leave
  alone.
- **Permission prompts:** Claude Code starts in its auto mode (PANE-18),
  for task agents and chats as for the Henchman, so the prompts it would
  otherwise repeat mostly never come.
- **Interrupted:** agents mid-turn when Layer quit are told to go on once
  they are back; one whose process ended by itself is restarted on its
  conversation and told to go on, once; a turn that ended on an error a
  retry fixes is told to go on (HEN-10).

