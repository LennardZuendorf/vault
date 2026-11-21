/**
 * Routines and their runs (§29) for the mock harness: a morning brief with
 * a run caught up after Layer was closed, an hourly sweep at work, and one
 * left off, in a world with tasks; none in the empty one.
 */
import { emit } from "@tauri-apps/api/event";
import type { PaneInfo } from "../lib/types";
import type { Routine, RoutinePatch, RoutineRun, Routines } from "../lib/types-routines";

type Args = Record<string, unknown>;

/** Starts a run's chat: an ACP pane in the run's room, sent the prompt. */
type Spawn = (r: Routine, room: string, ask: boolean) => PaneInfo;

const ROOMS = "/Users/you/.villain-worktrees/_chat/routines";

export function routineAnswers(seeded: boolean, spawn: Spawn) {
  const at = (days: number, hh: number, mm = 0) => {
    const d = new Date();
    d.setDate(d.getDate() + days);
    d.setHours(hh, mm, 0, 0);
    return d.toISOString();
  };
  const routines: Routine[] = seeded
    ? [
        {
          id: "r-brief",
          name: "Morning brief",
          prompt:
            "Read work_overview. Tell me what needs me today, at most five lines, the most pressing first: agents asking, PRs with changes requested or failed checks, tickets assigned to me that nobody started. Then one line for everything else.",
          every: "weekdays",
          at: "08:00",
          weekday: 0,
          agent: { agent_id: "claude", model: "sonnet", effort: "medium" },
          on: true,
          last_due: at(0, 8),
          next: at(1, 8),
        },
        {
          id: "r-sweep",
          name: "PR sweep",
          prompt:
            "List my open pull requests with list_prs. For each with failed checks or changes asked, say which and why in one line.",
          every: "hourly",
          at: "00:15",
          weekday: 0,
          agent: { agent_id: "claude", model: "", effort: "" },
          on: true,
          last_due: at(0, new Date().getHours(), 15),
          next: at(0, new Date().getHours() + 1, 15),
        },
        {
          id: "r-deps",
          name: "Dependency check",
          prompt: "",
          every: "weekly",
          at: "09:00",
          weekday: 0,
          agent: { agent_id: "claude", model: "", effort: "" },
          on: false,
          last_due: null,
          next: null,
        },
      ]
    : [];
  const run = (r: Partial<RoutineRun> & Pick<RoutineRun, "id" | "routine_id">): RoutineRun => ({
    due: null,
    started: at(0, 8),
    caught_up: false,
    room: `${ROOMS}/${r.routine_id}/${r.id}`,
    pane_id: null,
    session: null,
    state: "done",
    worked: false,
    said: "",
    ended: null,
    ...r,
  });
  const runs: RoutineRun[] = seeded
    ? [
        run({
          id: "run-sweep-1",
          routine_id: "r-sweep",
          due: at(0, new Date().getHours(), 15),
          started: at(0, new Date().getHours(), 15),
          state: "working",
        }),
        run({
          id: "run-brief-3",
          routine_id: "r-brief",
          due: at(0, 8),
          started: at(0, 9, 12),
          caught_up: true,
          worked: true,
          said: "3 need you: ACME-130 asks about the migration, ACME-123 failed checks, ACME-141 is yours and not started.",
          ended: at(0, 9, 14),
        }),
        run({
          id: "run-brief-2",
          routine_id: "r-brief",
          due: at(-1, 8),
          started: at(-1, 8),
          worked: true,
          said: "Nothing needs you. 4 PRs in review.",
          ended: at(-1, 8, 2),
        }),
        run({
          id: "run-brief-1",
          routine_id: "r-brief",
          due: at(-2, 8),
          started: at(-2, 8),
          state: "stopped",
          worked: true,
          said: "",
          ended: at(-2, 8, 1),
        }),
        run({
          id: "run-sweep-0",
          routine_id: "r-sweep",
          due: at(0, new Date().getHours() - 1, 15),
          started: at(0, new Date().getHours() - 1, 15),
          state: "failed",
          room: "",
          said: "Claude Code could not start: it is not signed in.",
        }),
      ]
    : [];

  const view = (): Routines => structuredClone({ routines, runs });
  const changed = () => void emit("routines:changed", view());
  const find = (id: unknown) => {
    const r = routines.find((x) => x.id === id);
    if (!r) throw new Error("That routine is gone.");
    return r;
  };
  const start = (r: Routine, due: string | null): RoutineRun => {
    if (runs.some((x) => x.routine_id === r.id && (x.state === "working" || x.state === "asking")))
      throw new Error(`${r.name} is still at work on its last run.`);
    if (!r.prompt.trim()) throw new Error(`${r.name} has nothing to ask yet.`);
    const id = `run-${Date.now()}`;
    const room = `${ROOMS}/${r.id}/${id}`;
    const p = spawn(r, room, true);
    const made = run({ id, routine_id: r.id, due, started: new Date().toISOString(), room });
    made.pane_id = p.id;
    made.state = "working";
    runs.unshift(made);
    changed();
    return made;
  };

  // The sweep's run at work has its conversation open: opened at the first
  // read, since the harness's events are not there yet while it boots.
  let opened = false;
  const openLive = () => {
    if (opened) return;
    opened = true;
    const live = runs.find((x) => x.state === "working");
    const sweep = routines.find((r) => r.id === live?.routine_id);
    if (!live || !sweep) return;
    live.pane_id = spawn(sweep, live.room, true).id;
    // So the window reads its panes again.
    void emit("pty:activity", live.pane_id);
  };

  return {
    routines_list: () => {
      openLive();
      return view();
    },
    routine_create: () => {
      const r: Routine = {
        id: `r-${Date.now()}`,
        name: "New routine",
        prompt: "",
        every: "daily",
        at: "08:00",
        weekday: 0,
        agent: { agent_id: "claude", model: "sonnet", effort: "medium" },
        on: true,
        last_due: null,
        next: at(1, 8),
      };
      routines.push(r);
      changed();
      return r.id;
    },
    routine_update: (a: Args) => {
      const r = find(a.id);
      const p = a.patch as RoutinePatch;
      if (p.at !== undefined && !/^([01]\d|2[0-3]):[0-5]\d$/.test(p.at))
        throw new Error(`“${p.at}” is not a time: write it as 08:00.`);
      if (p.name !== undefined) r.name = p.name.trim() || r.name;
      if (p.prompt !== undefined) r.prompt = p.prompt;
      if (p.every !== undefined) r.every = p.every;
      if (p.at !== undefined) r.at = p.at;
      if (p.weekday !== undefined) r.weekday = p.weekday;
      if (p.agent_id !== undefined) r.agent = { agent_id: p.agent_id, model: "", effort: "" };
      if (p.model !== undefined) r.agent.model = p.model;
      if (p.effort !== undefined) r.agent.effort = p.effort;
      if (p.on !== undefined) r.on = p.on;
      r.next = r.on ? at(1, Number(r.at.slice(0, 2)), Number(r.at.slice(3))) : null;
      changed();
      return null;
    },
    routine_delete: (a: Args) => {
      const r = find(a.id);
      routines.splice(routines.indexOf(r), 1);
      for (let i = runs.length - 1; i >= 0; i--) if (runs[i].routine_id === r.id) runs.splice(i, 1);
      changed();
      return null;
    },
    routine_run_now: (a: Args) => start(find(a.id), null),
    routine_open_run: (a: Args) => {
      const x = runs.find((r) => r.id === a.runId);
      if (!x || !x.room) throw new Error("That run never started, so it has no conversation.");
      const p = spawn(find(x.routine_id), x.room, false);
      p.activity = "done";
      x.pane_id = p.id;
      changed();
      return p;
    },
  };
}
