import { getCurrentWindow } from "@tauri-apps/api/window";
/**
 * Going where a banner, a toast or a message points (NOTE-4): the one place
 * that turns a target into a view, a task, a tab and a selection.
 */
import { useEffect } from "react";
import { useStore } from "../store";
import { api } from "./api";
import { isHenchman } from "./lair";
import { isRoutineRun, useRoutines } from "./routines";
import { route } from "./target";

/**
 * Open what `target` is about. `raise` brings the window forward too, for a
 * banner clicked while the app was behind something else.
 */
export function goTo(target: string, opts: { raise?: boolean } = {}) {
  const s = useStore.getState();
  const step = route(target, { tasks: s.tasks.map((t) => t.id), panes: s.panes });
  if (step) {
    // Settings is a sheet over every view: left open, it hides where this went.
    if (s.settingsOpen) s.toggleSettings(false);
    switch (step.view) {
      case "work":
        s.select(step.task);
        // After select, which always lands on the terminals.
        if (step.tab) s.setTab(step.tab);
        if (step.pane) s.showPane(step.pane);
        break;
      case "chat":
        // The Henchman is a chat with a tab of its own (HEN-2): its banners open it there.
        if (step.pane && s.panes.some((p) => p.id === step.pane && isHenchman(p))) {
          s.setView("henchman");
          break;
        }
        // So is a routine's run (ROUT-1): its own conversation, on its routine.
        if (step.pane) {
          const run = useRoutines.getState().routines?.runs.find((r) => r.pane_id === step.pane);
          if (run || s.panes.some((p) => p.id === step.pane && isRoutineRun(p))) {
            s.setView("routines");
            if (run) {
              useRoutines.getState().show(run.routine_id);
              useRoutines.getState().showRun(run.id);
            }
            break;
          }
        }
        s.setView("chat");
        if (step.pane) s.showPane(step.pane);
        break;
      case "routines":
        s.setView("routines");
        if (step.routine) useRoutines.getState().show(step.routine);
        break;
      case "reviews":
        if (step.review) s.showReview(step.review);
        else s.setView("reviews");
        break;
      case "tickets":
        if (step.issue) s.showIssue(step.issue);
        else s.setView("tickets");
        break;
    }
  }
  // Whatever else was said about the same thing has been seen now too.
  const seen = s.messages.filter((m) => !m.read && m.target === target).map((m) => m.id);
  if (seen.length) void api.markMessagesRead(seen).catch(() => {});
  if (opts.raise) {
    const win = getCurrentWindow();
    void win.unminimize().catch(() => {});
    void win.show().catch(() => {});
    void win.setFocus().catch(() => {});
  }
}

/**
 * Select the pane a click asked for, once it is among `panes`. Declared after
 * a view's own "keep a sensible pane selected" effect, so on mount this one's
 * choice is the one that stands.
 */
export function useFocusedPane(ids: string[], select: (id: string) => void) {
  const focus = useStore((s) => s.focusPane);
  const clear = useStore((s) => s.clearFocusPane);
  useEffect(() => {
    if (!focus) return;
    if (ids.includes(focus)) {
      select(focus);
      clear();
    } else if (!useStore.getState().panes.some((p) => p.id === focus)) {
      // Gone: nothing will ever claim it.
      clear();
    }
  }, [focus, ids, select, clear]);
}
