import { useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { api } from "../../lib/api";
import { isHenchman } from "../../lib/lair";
import type { WaitingPane } from "../../lib/types";
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
  const chosen = useStore((s) => s.settings?.henchman.agent_id);
  // The one chosen for it (HEN-15), else any that runs as a conversation.
  const usable = agents.filter((a) => a.installed && a.acp);
  const agent = usable.find((a) => a.id === chosen) ?? usable[0];
  const asleep = useStore(useShallow((s) => s.waiting.filter(isWaitingHenchman)));
  // Off in Settings (HEN-16): the tab is the lair alone. On until settings say.
  const off = useStore((s) => s.settings?.henchman_on === false);
  const toggleSettings = useStore((s) => s.toggleSettings);

  async function wake() {
    // One not put back at launch (PANE-7) comes back with its conversation.
    // Forgotten and replaced, it began a new one at every restart that left
    // it waiting.
    const left = useStore.getState().waiting.filter(isWaitingHenchman);
    const back = left[left.length - 1];
    if (waking || (!back && !agent)) return;
    setWaking(true);
    failure.clear();
    try {
      const forget = (ids: string[]) =>
        Promise.all(ids.map((id) => api.forgetWaitingPane(id).catch(() => {})));
      await forget(left.filter((w) => w !== back).map((w) => w.id));
      if (back) {
        // One that cannot come back is let go, so the next click wakes a
        // new one instead of failing again.
        await api.reopenWaitingPane(back.id).catch(async (e: unknown) => {
          await forget([back.id]);
          throw e;
        });
      } else if (agent) {
        await api.spawnHenchman(agent.id);
      }
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
        {off ? (
          <div className="hench-empty">
            {hello}
            <p className="muted">
              The Henchman is off. Your agents ping you themselves, and the lair beside this lists
              what needs you.
            </p>
            <button className="ui-button" onClick={() => toggleSettings(true, "agents")}>
              Turn it on in Settings
            </button>
          </div>
        ) : pane ? (
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
              disabled={(!agent && asleep.length === 0) || waking}
              onClick={() => void wake()}
            >
              {waking
                ? "Waking…"
                : asleep.length > 0
                  ? "Wake the Henchman where it left off"
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

/** A Henchman not put back at launch (PANE-7), waiting to be reopened. */
function isWaitingHenchman(w: WaitingPane): boolean {
  return w.cwd !== null && isHenchman({ task_id: w.task_id, cwd: w.cwd });
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
