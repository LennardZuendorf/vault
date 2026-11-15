import { CHAT_TASK_ID, taskTotals } from "./derive";
import type { CheckoutPr, PaneInfo, TaskView } from "./types";

/**
 * The lair (HEN-5): every task sorted by what it needs. The same rule as
 * `sort` in `commands/henchman_tools.rs`, which answers `work_overview`, so
 * the column and the Henchman's brief never disagree.
 */
export type LairGroup = "needs_you" | "to_finish" | "moving" | "not_started" | "quiet";

export const LAIR_GROUPS: { id: LairGroup; label: string; open: boolean }[] = [
  { id: "needs_you", label: "Needs you", open: true },
  { id: "to_finish", label: "To finish", open: true },
  { id: "moving", label: "Moving", open: false },
  { id: "not_started", label: "Not started", open: false },
  { id: "quiet", label: "Quiet", open: false },
];

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
  /** Some PR open or merged, and every one of them merged. */
  merged: boolean;
  open: number;
  approved: boolean;
  agents: boolean;
  prs: boolean;
  changed: number;
}

const FAILED = new Set(["failure", "timed_out", "action_required", "startup_failure"]);

export function facts(panes: PaneInfo[], rows: CheckoutPr[], changed: number): Facts {
  const live = panes.filter((p) => p.kind === "agent" && p.running);
  const budget = (p: PaneInfo) => p.notice === "usage_limit";
  const open = rows.filter((r) => r.pr?.state === "open");
  const landed = rows.filter((r) => r.pr && (r.pr.state === "open" || r.pr.merged));
  return {
    asking: live.some((p) => p.activity === "asking" && !budget(p)),
    outOfBudget: live.some(budget),
    finished: live.some((p) => p.activity === "done"),
    working: live.some((p) => p.activity === "working"),
    changesRequested: open.some((r) => r.verdict === "changes_requested"),
    checksFailed: open.some((r) => r.checks.some((c) => FAILED.has(c.conclusion ?? ""))),
    merged: landed.length > 0 && landed.every((r) => r.pr!.merged),
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
  if (!f.agents && !f.prs && f.changed === 0)
    return { group: "not_started", rank: 0, why: "not started" };
  return { group: "quiet", rank: 0, why: f.agents ? "its agent is idle" : "nothing running" };
}

export interface LairRow {
  task: TaskView;
  why: string;
  rank: number;
}

export interface Lair {
  groups: Record<LairGroup, LairRow[]>;
  /** Chats other than the Henchman, those asking first. */
  chats: PaneInfo[];
  /** Chats that ask you: they count in Needs you. */
  chatsAsking: number;
}

export function lair(
  tasks: TaskView[],
  panes: PaneInfo[],
  prs: Record<string, CheckoutPr[]>,
): Lair {
  const groups: Record<LairGroup, LairRow[]> = {
    needs_you: [],
    to_finish: [],
    moving: [],
    not_started: [],
    quiet: [],
  };
  for (const task of tasks) {
    const t = taskTotals(task);
    const own = panes.filter((p) => p.task_id === task.id);
    const sorted = sortTask(
      facts(own, prs[task.id] ?? [], t.changed + t.ahead + t.staged + t.dirty),
    );
    groups[sorted.group].push({ task, why: sorted.why, rank: sorted.rank });
  }
  groups.needs_you.sort((a, b) => a.rank - b.rank);
  const asks = (p: PaneInfo) => p.running && p.activity === "asking";
  const chats = panes
    .filter((p) => p.task_id === CHAT_TASK_ID && p.kind === "agent" && !isHenchman(p))
    .sort((a, b) => Number(asks(b)) - Number(asks(a)));
  return { groups, chats, chatsAsking: chats.filter(asks).length };
}

let needsSeen: {
  tasks: TaskView[];
  panes: PaneInfo[];
  prs: Record<string, CheckoutPr[]>;
  n: number;
} | null = null;

/**
 * How many need you, for the top bar's Henchman tab: worked out again only
 * when the tasks, panes or PRs change. It is read in a selector, which runs on
 * every change of anything in the store, and the lair walks every task's panes.
 */
export function lairNeeds(
  tasks: TaskView[],
  panes: PaneInfo[],
  prs: Record<string, CheckoutPr[]>,
): number {
  const seen = needsSeen;
  if (seen && seen.tasks === tasks && seen.panes === panes && seen.prs === prs) return seen.n;
  const n = lairCounts(lair(tasks, panes, prs)).needsYou;
  needsSeen = { tasks, panes, prs, n };
  return n;
}

/** "20 open · 3 need you · 1 to finish · …": the line under the title. */
export function lairCounts(l: Lair): { open: number; needsYou: number; line: string } {
  const g = l.groups;
  const open = Object.values(g).reduce((n, rows) => n + rows.length, 0);
  const needsYou = g.needs_you.length + l.chatsAsking;
  const parts = [
    `${open} open`,
    `${needsYou} need${needsYou === 1 ? "s" : ""} you`,
    `${g.to_finish.length} to finish`,
    `${g.moving.length} moving`,
    `${g.not_started.length} not started`,
    `${g.quiet.length} quiet`,
  ];
  return { open, needsYou, line: parts.join(" · ") };
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
