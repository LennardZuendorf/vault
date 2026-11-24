import { create } from "zustand";
import { useStore } from "../store";
import { api, failureText } from "./api";
import { taskReview, type TaskReview } from "./derive";
import type { EpicOf } from "./grouping";
import type { CheckoutPr, EpicView, PaneInfo, SubTaskView, TaskView } from "./types";

/** Where a sub-task stands, in the order EPIC-10 asks the questions. */
export type SubTaskState =
  | "merged"
  | "changes"
  | "approved"
  | "in review"
  | "asking"
  | "working"
  | "waits"
  | "ready"
  | "to file";

/**
 * The first that holds (EPIC-10): its pull requests, as the task list reads
 * them; then its agents; then what it waits for. A sub-task not filed yet
 * has no task to ask about.
 */
export function subTaskState(
  filed: boolean,
  review: TaskReview,
  agents: { waiting: boolean; live: number },
  waits: boolean,
): SubTaskState {
  if (!filed) return "to file";
  if (review === "merged") return "merged";
  if (review === "changes_requested") return "changes";
  if (review === "approved") return "approved";
  if (review !== "none") return "in review";
  if (agents.waiting) return "asking";
  if (agents.live > 0) return "working";
  if (waits) return "waits";
  return "ready";
}

/** Its word's colour: calm for done and waiting, a status where it needs someone. */
export const STATE_COLOR: Partial<Record<SubTaskState, string>> = {
  changes: "var(--status-danger-ink)",
  approved: "var(--status-ok-ink)",
  asking: "var(--status-warn-ink)",
};

/**
 * The titles `sub` waits for that have not merged. One not filed has not;
 * read from the pull requests the task list already has.
 */
export function openWaits(
  sub: SubTaskView,
  all: SubTaskView[],
  prs: Record<string, CheckoutPr[]>,
): string[] {
  return sub.waits_for.filter((title) => {
    const other = all.find((s) => s.title.toLowerCase() === title.toLowerCase());
    return !other?.task_id || taskReview(prs[other.task_id] ?? []) !== "merged";
  });
}

/**
 * Each epic's Sub-tasks tab, as last read. Read when its row or page is
 * drawn and again after anything that changes it (filing, writing its
 * tasks); its sub-tasks' own progress comes from the task list's events.
 */
export const useEpicViews = create<{
  byEpic: Record<string, EpicView>;
  /** The ticket of the epic whose chat starts once its page shows (EPIC-11). */
  chatFor: string | null;
  load: (epicId: string) => Promise<void>;
  put: (epicId: string, view: EpicView) => void;
}>((set) => ({
  byEpic: {},
  chatFor: null,
  load: async (epicId) => {
    const view = await api.epicView(epicId);
    set((s) => ({ byEpic: { ...s.byEpic, [epicId]: view } }));
  },
  put: (epicId, view) => set((s) => ({ byEpic: { ...s.byEpic, [epicId]: view } })),
}));

/**
 * Make a Jira epic one here, in `repos`, its tasks' repositories, and open it
 * on Sub-tasks (EPIC-13). Its page shows meanwhile, "Opening…" where its
 * sub-tasks will be, and why when it could not be made. No agent starts and
 * its ticket stays put (EPIC-2).
 */
export async function openJiraEpic(epic: EpicOf, repos: string[]) {
  const { opening, select } = useStore.getState();
  if (opening?.epic.key === epic.key && !opening.error) return;
  select(null);
  useStore.setState({ opening: { epic, repos, error: null } });
  // Something picked meanwhile keeps its place, and is not told of a failure.
  const still = () => useStore.getState().opening?.epic.key === epic.key;
  try {
    const made = await api.startWorkOnTicket(epic.key, repos, null, null, null, true);
    await useStore.getState().refreshTasks();
    if (still()) select(made.id);
  } catch (e) {
    if (still()) useStore.setState({ opening: { epic, repos, error: failureText(e) } });
  }
}

/**
 * Whether the ticket `key`'s menu offers Start the epic's chat (EPIC-11): no
 * task has it yet, or its task is an epic with no chat. A task on it that is
 * no epic (EPIC-13) has no chat to start.
 */
export function epicChatOffered(
  key: string,
  tasks: readonly Pick<TaskView, "id" | "issue_key" | "epic">[],
  panes: readonly Pick<PaneInfo, "task_id" | "kind">[],
): boolean {
  const task = tasks.find((t) => t.issue_key === key);
  if (!task) return true;
  return !!task.epic && !panes.some((p) => p.task_id === task.id && p.kind === "agent");
}

/**
 * Open the epic on `epic.key` with its chat starting (EPIC-11), from its menu
 * in Tickets. Its chat column starts it, so its own Start button says
 * "Starting…" rather than offering a second. A Jira epic that is no epic here
 * yet is made one first, in the repositories Start work would pick for it;
 * `noRepos` when there are none to pick.
 */
export async function chatWithEpic(epic: EpicOf, noRepos: () => void) {
  const { tasks, select } = useStore.getState();
  const made = tasks.find((t) => t.epic && t.issue_key === epic.key);
  const repos = made
    ? []
    : await api.suggestRepos({ issueKey: epic.key, epicKey: epic.key }).then(
        (s) => s.project_ids,
        () => [],
      );
  if (!made && repos.length === 0) return noRepos();
  useEpicViews.setState({ chatFor: epic.key });
  if (made) select(made.id);
  else await openJiraEpic(epic, repos);
}
