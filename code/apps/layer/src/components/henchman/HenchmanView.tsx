import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { api } from "../../lib/api";
import { isHenchman } from "../../lib/lair";
import { useStore } from "../../store";
import { PaneView } from "../AcpPane";
import { PaneExited, useRestart } from "../Restart";
import { ErrorMessage, useFailure } from "../ui";
import { LairColumn } from "./Lair";

/**
 * The Henchman's tab (§28): its conversation on the left, the lair beside
 * it. The Henchman is a chat in a room of its own (HEN-2), so everything a
 * chat has (restore, banners, runs) it has as well.
 */
export function HenchmanView() {
  const panes = useStore(useShallow((s) => s.panes.filter(isHenchman)));
  const agents = useStore((s) => s.agents);
  const refreshPanes = useStore((s) => s.refreshPanes);
  const failure = useFailure();
  const [waking, setWaking] = useState(false);
  const { restart, restarting } = useRestart(() => {});
  // The one running, else the last: an exited one says so and offers Resume.
  const pane = panes.find((p) => p.running) ?? panes[panes.length - 1];
  const agent = agents.find((a) => a.installed && a.acp);

  async function wake() {
    if (!agent || waking) return;
    setWaking(true);
    failure.clear();
    try {
      // One not put back at launch (PANE-7) is replaced, not kept waiting
      // where nothing shows it.
      const left = useStore
        .getState()
        .waiting.filter((w) => w.cwd && isHenchman({ task_id: w.task_id, cwd: w.cwd }));
      await Promise.all(left.map((w) => api.forgetWaitingPane(w.id).catch(() => {})));
      await api.spawnHenchman(agent.id);
      await Promise.all([refreshPanes(), useStore.getState().refreshWaiting()]);
    } catch (e) {
      failure.fail(e);
    } finally {
      setWaking(false);
    }
  }

  const since = pane ? new Date(pane.started_at) : null;
  const hello = <Hello at={pane?.running ? since : null} />;

  return (
    <div className="hench">
      <div className="hench-talk">
        {pane ? (
          <>
            <PaneExited pane={pane} busy={restarting.has(pane.id)} onResume={restart} />
            <div className="pane-stack">
              <PaneView
                pane={pane}
                visible
                head={hello}
                ask="Ask the Henchman, or tell it what to do…"
              />
            </div>
          </>
        ) : (
          <div className="hench-empty">
            {hello}
            <p className="muted">
              It reads every task, chat and agent in Layer, briefs you on what needs you, and passes
              your words on to any of them. It writes no code.
            </p>
            <button
              className="ui-button ui-button-primary"
              disabled={!agent || waking}
              onClick={() => void wake()}
            >
              {waking
                ? "Waking…"
                : agent
                  ? `Wake the Henchman with ${agent.name}`
                  : "No agent that talks over ACP"}
            </button>
            <ErrorMessage>{failure.error}</ErrorMessage>
          </div>
        )}
      </div>
      <LairColumn />
    </div>
  );
}

/** The title above the conversation: when the Henchman woke. */
function Hello({ at }: { at: Date | null }) {
  const time = at?.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return (
    <div className="hench-hello">
      <h1 className="hench-title">{time ? `The lair at ${time}` : "The lair"}</h1>
    </div>
  );
}
