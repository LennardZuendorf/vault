import { create } from "zustand";
import { api } from "./api";
import { CHAT_TASK_ID } from "./derive";
import { read, write } from "./persist";
import type { PaneInfo } from "./types";
import type { Routine, RoutineRun, Routines } from "./types-routines";

/**
 * Routines and their runs (§29), kept by the backend on this Mac. Read
 * once, then replaced by each `routines:changed`, which carries them whole
 * (`Watchers.tsx`). Which routine is open, and which run on it, is the
 * window's own.
 */
interface RoutinesStore {
  routines: Routines | null;
  /** The routine on screen. */
  open: string | null;
  /** The run whose conversation is on screen, over the routine's page. */
  run: string | null;
  refresh: () => Promise<void>;
  set: (routines: Routines) => void;
  show: (id: string | null) => void;
  showRun: (id: string | null) => void;
}

export const useRoutines = create<RoutinesStore>((set) => ({
  routines: null,
  open: read<string | null>("openRoutine", null),
  run: null,
  refresh: async () => set({ routines: await api.routinesList() }),
  set: (routines) => set({ routines }),
  show: (id) => {
    write("openRoutine", id);
    set({ open: id, run: null });
  },
  showRun: (run) => set({ run }),
}));

/** A routine's run: a chat in a room under `_chat/routines` (ROUT-4). */
export function isRoutineRun(p: Pick<PaneInfo, "task_id" | "cwd">): boolean {
  return p.task_id === CHAT_TASK_ID && /[/\\]_chat[/\\]routines[/\\]/.test(p.cwd);
}

export const WEEKDAYS = [
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
  "Sunday",
] as const;

/** "Weekdays at 08:00", "Every hour at :15", "Mondays at 17:30". */
export function schedule(r: Pick<Routine, "every" | "at" | "weekday">): string {
  switch (r.every) {
    case "hourly":
      return `Every hour at :${r.at.split(":")[1] ?? "00"}`;
    case "daily":
      return `Every day at ${r.at}`;
    case "weekdays":
      return `Weekdays at ${r.at}`;
    case "weekly":
      return `${WEEKDAYS[r.weekday] ?? "Monday"}s at ${r.at}`;
  }
}

const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();
const hm = (d: Date) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** "Today, 08:00", "Yesterday, 08:00", "Wed, 08:00", "Mon 12 Oct, 08:00". */
export function day(iso: string, now = new Date()): string {
  const d = new Date(iso);
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  if (sameDay(d, now)) return `Today, ${hm(d)}`;
  if (sameDay(d, yesterday)) return `Yesterday, ${hm(d)}`;
  if (sameDay(d, tomorrow)) return `Tomorrow, ${hm(d)}`;
  const week = Math.abs(d.getTime() - now.getTime()) < 6 * 86_400_000;
  const date = d.toLocaleDateString(
    [],
    week ? { weekday: "short" } : { weekday: "short", day: "numeric", month: "short" },
  );
  return `${date}, ${hm(d)}`;
}

/** "2 min", "1 h 5 min", "under a minute". */
export function took(from: string, to: string): string {
  const min = Math.round((new Date(to).getTime() - new Date(from).getTime()) / 60_000);
  if (min < 1) return "under a minute";
  if (min < 60) return `${min} min`;
  return `${Math.floor(min / 60)} h${min % 60 ? ` ${min % 60} min` : ""}`;
}

/** How a run went, for the end of its row (ROUT-8). */
export function outcome(run: RoutineRun): string {
  const late = run.caught_up ? ` · caught up at ${hm(new Date(run.started))}` : "";
  switch (run.state) {
    case "working":
      return `working${late}`;
    case "asking":
      return `asking you${late}`;
    case "done":
      return `${run.ended ? took(run.started, run.ended) : "done"}${late}`;
    case "stopped":
      return "stopped";
    case "skipped":
      return "skipped";
    case "failed":
      return "failed";
  }
}

/** What the list says under a routine's name: how it is doing, or when it runs next. */
export function standing(r: Routine, last: RoutineRun | undefined): string {
  if (last?.state === "working") return "running";
  if (last?.state === "asking") return "asking you";
  if (!r.on) return "off";
  if (!r.prompt.trim()) return "no prompt yet";
  return r.next ? `next ${day(r.next)}` : schedule(r);
}
