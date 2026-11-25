/** How often a routine runs (ROUT-2). `Every` in `routines.rs`. */
export type Every = "hourly" | "daily" | "weekdays" | "weekly";

/** A routine as the window gets it: `RoutineView` in `routines.rs`. */
export interface Routine {
  id: string;
  name: string;
  prompt: string;
  every: Every;
  /** "HH:MM"; hourly reads only the minutes. */
  at: string;
  /** Weekly's day, 0 for Monday to 6 for Sunday. */
  weekday: number;
  agent: { agent_id: string; model: string; effort: string };
  on: boolean;
  last_due: string | null;
  /** When it runs next; null while it is off. */
  next: string | null;
}

export type RunState = "working" | "asking" | "done" | "stopped" | "skipped" | "failed";

/** One run of a routine (ROUT-8). `Run` in `routines.rs`. */
export interface RoutineRun {
  id: string;
  routine_id: string;
  /** The time it was due at; null for Run now. */
  due: string | null;
  started: string;
  caught_up: boolean;
  room: string;
  pane_id: string | null;
  session: string | null;
  state: RunState;
  worked: boolean;
  /** The first line of what its agent last said, or why it did not run. */
  said: string;
  ended: string | null;
}

/** Every routine, and every run kept, newest first. `View` in `routines.rs`. */
export interface Routines {
  routines: Routine[];
  runs: RoutineRun[];
}

/** What changed on a routine's page. `RoutinePatch` in `commands/routines.rs`. */
export interface RoutinePatch {
  name?: string;
  prompt?: string;
  every?: Every;
  at?: string;
  weekday?: number;
  agent_id?: string;
  model?: string;
  effort?: string;
  on?: boolean;
}
