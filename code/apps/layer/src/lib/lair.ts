import { CHAT_TASK_ID, paneName, taskTotals } from "./derive";
import { isRoutineRun } from "./routines";
import type { CheckoutPr, PaneInfo, TaskView } from "./types";

/**
 * How each task stands (HEN-5), by the same rule as `sort` in
 * `commands/henchman_tools.rs`, which answers `work_overview`: the lair
 * and the Henchman's brief never disagree.
 */
export type LairGroup = "needs_you" | "to_finish" | "moving" | "not_started" | "quiet";

/** The Henchman's pane: a chat in its own room (HEN-2). */
export function isHenchman(p: Pick<PaneInfo, "task_id" | "cwd">): boolean {
  return p.task_id === CHAT_TASK_ID && /[/\\]_chat[/\\]henchman[/\\]?$/.test(p.cwd);
}

export interface Facts {
  asking: boolean;
  outOfBudget: boolean;
  finished: boolean;
  working: boolean;
  changesRequested: boolean;
  checksFailed: boolean;
  /** Some PR open or merged, and every one of them merged; for an epic, every sub-task's too. */
  merged: boolean;
  /** An epic with a sub-task whose pull requests have not all merged. */
  subTasksOpen: boolean;
  open: number;
  approved: boolean;
  agents: boolean;
  prs: boolean;
  changed: number;
}

const FAILED = new Set(["failure", "timed_out", "action_required", "startup_failure"]);

/** `subTasks` is whether each of an epic's sub-tasks has merged; empty for any other task. */
export function facts(
  panes: PaneInfo[],
  rows: CheckoutPr[],
  changed: number,
  subTasks: boolean[] = [],
): Facts {
  const live = panes.filter((p) => p.kind === "agent" && p.running);
  const budget = (p: PaneInfo) => p.notice === "usage_limit";
  const open = rows.filter((r) => r.pr?.state === "open");
  const landed = rows.filter((r) => r.pr && (r.pr.state === "open" || r.pr.merged));
  const subTasksOpen = subTasks.some((m) => !m);
  return {
    asking: live.some((p) => p.activity === "asking" && !budget(p)),
    outOfBudget: live.some(budget),
    finished: live.some((p) => p.activity === "done"),
    working: live.some((p) => p.activity === "working"),
    changesRequested: open.some((r) => r.verdict === "changes_requested"),
    checksFailed: open.some((r) => r.checks.some((c) => FAILED.has(c.conclusion ?? ""))),
    // An epic's own pull request merging said "finish the task" while its
    // sub-tasks were still in review: it is merged once they all are.
    merged:
      subTasks.length === 0
        ? landed.length > 0 && landed.every((r) => r.pr!.merged)
        : open.length === 0 && !subTasksOpen,
    subTasksOpen,
    open: open.length,
    approved: open.length > 0 && open.every((r) => r.verdict === "approved"),
    agents: live.length > 0,
    prs: rows.some((r) => r.pr),
    changed: Math.max(changed, ...rows.map((r) => r.changed)),
  };
}

/** The group, a rank within Needs you (lower first), and why. */
export function sortTask(f: Facts): { group: LairGroup; rank: number; why: string } {
  if (f.asking) return { group: "needs_you", rank: 0, why: "its agent asks" };
  if (f.outOfBudget) return { group: "needs_you", rank: 1, why: "its agent is out of budget" };
  if (f.finished) return { group: "needs_you", rank: 2, why: "its agent finished: your turn" };
  if (!f.working && f.changesRequested)
    return { group: "needs_you", rank: 3, why: "changes asked for" };
  if (!f.working && f.checksFailed) return { group: "needs_you", rank: 4, why: "checks failed" };
  if (f.merged) return { group: "to_finish", rank: 0, why: "merged" };
  if (f.working) return { group: "moving", rank: 0, why: "its agent is working" };
  if (f.open > 0) return { group: "moving", rank: 0, why: f.approved ? "approved" : "in review" };
  if (f.subTasksOpen) return { group: "moving", rank: 0, why: "its sub-tasks are not all merged" };
  if (!f.agents && !f.prs && f.changed === 0)
    return { group: "not_started", rank: 0, why: "not started" };
  return { group: "quiet", rank: 0, why: f.agents ? "its agent is idle" : "nothing running" };
}

/** One thing in the tasks and chats that only the user can do (HEN-5). */
export interface Waiting {
  id: string;
  /** The ticket key, the task's name, or "Chat". */
  key: string;
  why: string;
  task: TaskView | null;
  /** The agent or chat that waits, which Open opens. */
  pane: PaneInfo | null;
  /** Every pull request merged: Finish, not Open. */
  finish: boolean;
}

const WAITS = ["asking", "usage_limit", "done"] as const;

/**
 * What waits for the user, the most pressing first: an agent's or a chat's
 * question, an agent out of budget, a task whose pull requests all merged.
 * While the Henchman runs, the rest of Needs you in `sortTask` (a finished
 * turn, changes asked for, checks failed) is its to handle; with none,
 * nobody else will, so they are the user's too.
 */
export function waiting(
  tasks: TaskView[],
  panes: PaneInfo[],
  prs: Record<string, CheckoutPr[]>,
): Waiting[] {
  const henchman = panes.some((p) => p.running && isHenchman(p));
  const subTasks = new Map<string, boolean[]>();
  for (const t of tasks) {
    if (!t.sub_task) continue;
    const epic = t.sub_task.epic;
    subTasks.set(epic, [...(subTasks.get(epic) ?? []), facts([], prs[t.id] ?? [], 0).merged]);
  }
  const out: { w: Waiting; rank: number }[] = [];
  for (const task of tasks) {
    const own = panes.filter((p) => p.task_id === task.id);
    const t = taskTotals(task);
    const changed = t.changed + t.ahead + t.staged + t.dirty;
    const subs = task.epic ? (subTasks.get(task.id) ?? []) : [];
    const s = sortTask(facts(own, prs[task.id] ?? [], changed, subs));
    const finish = s.group === "to_finish";
    if (!finish && !(s.group === "needs_you" && (!henchman || s.rank <= 1))) continue;
    const live = own.filter((p) => p.kind === "agent" && p.running);
    const pane =
      WAITS.map((w) =>
        live.find((p) => (w === "usage_limit" ? p.notice === w : p.activity === w)),
      ).find(Boolean) ?? null;
    const key = task.issue_key ?? task.name;
    const why = finish ? "merged: finish the task" : s.why;
    out.push({ w: { id: task.id, key, why, task, pane, finish }, rank: finish ? 9 : s.rank });
  }
  for (const p of panes) {
    if (p.task_id !== CHAT_TASK_ID || p.kind !== "agent" || isHenchman(p) || isRoutineRun(p))
      continue;
    if (!p.running || p.activity !== "asking") continue;
    const w = { id: p.id, key: "Chat", why: `“${paneName(p)}” asks you`, task: null, pane: p };
    out.push({ w: { ...w, finish: false }, rank: 0.5 });
  }
  return out.sort((a, b) => a.rank - b.rank).map((o) => o.w);
}

let waitingSeen: {
  tasks: TaskView[];
  panes: PaneInfo[];
  prs: Record<string, CheckoutPr[]>;
  n: number;
} | null = null;

/**
 * How many tasks and chats wait for the user, for the top bar's Henchman
 * tab, beside the Henchman's own needs: worked out again only when the
 * tasks, panes or PRs change. It is read in a selector, which runs on every
 * change of anything in the store, and it walks every task's panes.
 */
export function waitingCount(
  tasks: TaskView[],
  panes: PaneInfo[],
  prs: Record<string, CheckoutPr[]>,
): number {
  const seen = waitingSeen;
  if (seen && seen.tasks === tasks && seen.panes === panes && seen.prs === prs) return seen.n;
  const n = waiting(tasks, panes, prs).length;
  waitingSeen = { tasks, panes, prs, n };
  return n;
}

/** "12s", "4m", "2h 5m", "3d": how long a process has run. */
export function ran(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) {
    const m = Math.floor((secs % 3600) / 60);
    return `${Math.floor(secs / 3600)}h${m ? ` ${m}m` : ""}`;
  }
  return `${Math.floor(secs / 86400)}d`;
}

/**
 * A command with each path cut to its last part: "node …/vite --port 5173".
 * The worktree's path was all a narrow row had room for. The whole command
 * is the row's tooltip.
 */
export function shortCommand(command: string): string {
  return command
    .split(" ")
    .map((w) => (/^\/.+\/[^/]+$/.test(w) ? `…/${w.slice(w.lastIndexOf("/") + 1)}` : w))
    .join(" ");
}
