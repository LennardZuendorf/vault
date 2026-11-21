import { useState } from "react";
import { api } from "../../lib/api";
import { day, outcome, useRoutines } from "../../lib/routines";
import type { Routine, RoutineRun } from "../../lib/types-routines";
import { useStore } from "../../store";
import { PaneView } from "../AcpPane";
import { BusyLabel, ErrorMessage, useFailure } from "../ui";

/**
 * One run's conversation (ROUT-8, ROUT-9). Only the newest run keeps its
 * agent open; an older one, or one whose agent has exited, opens again on
 * its own conversation.
 */
export function RunConversation({ routine, run }: { routine: Routine; run: RoutineRun }) {
  const pane = useStore((s) => s.panes.find((p) => p.id === run.pane_id));
  const refreshPanes = useStore((s) => s.refreshPanes);
  const showRun = useRoutines((s) => s.showRun);
  const failure = useFailure();
  const [opening, setOpening] = useState(false);

  async function reopen() {
    if (opening) return;
    setOpening(true);
    failure.clear();
    try {
      await api.routineOpenRun(run.id);
      await Promise.all([useRoutines.getState().refresh(), refreshPanes()]);
    } catch (e) {
      failure.fail(e);
    } finally {
      setOpening(false);
    }
  }

  const when = run.due ? day(run.due) : `Run now, ${day(run.started)}`;
  return (
    <div className="routine-run-view">
      <div className="routine-run-bar">
        <button className="ui-link" onClick={() => showRun(null)}>
          ← {routine.name}
        </button>
        <span className="muted">{when}</span>
        <span className="muted">· {outcome(run)}</span>
      </div>
      <ErrorMessage>{failure.error}</ErrorMessage>
      {pane && !pane.running && (
        <div className="limit-banner">
          <span>This run&apos;s agent has stopped. Open it again to ask about what it did.</span>
          <div className="spacer" />
          <button
            className="ui-button ui-button-primary"
            disabled={opening}
            onClick={() => void reopen()}
          >
            <BusyLabel busy={opening} idle="Open again" working="Opening…" />
          </button>
        </div>
      )}
      {pane ? (
        <div className="pane-stack">
          <PaneView pane={pane} visible />
        </div>
      ) : (
        <div className="empty">
          <p>{run.said || "This run's conversation is closed."}</p>
          <div className="row">
            <button
              className="ui-button ui-button-primary"
              disabled={opening}
              onClick={() => void reopen()}
            >
              <BusyLabel busy={opening} idle="Open its conversation" working="Opening…" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
