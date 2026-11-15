import { useEffect, useMemo, useState } from "react";
import { api } from "../../lib/api";
import { CHAT_TASK_ID, paneName } from "../../lib/derive";
import { goTo } from "../../lib/goto";
import { isHenchman, ran, shortCommand } from "../../lib/lair";
import type { Process } from "../../lib/types-henchman";
import { useStore } from "../../store";
import { ErrorMessage, useFailure } from "../ui";

/**
 * What is at work now (HEN-6): each agent in a turn, and what the panes
 * started that still runs (dev servers, builds, watchers). Shown only when
 * there is any. The processes are read when a pane's state moves, and
 * every 10 seconds while the window is active, since a process starts and
 * ends without telling anyone; that read also keeps the times current.
 */
export function AtWork() {
  const appActive = useStore((s) => s.appActive);
  const panes = useStore((s) => s.panes);
  const tasks = useStore((s) => s.tasks);
  // What a pane does, not its output: a server started or stopped moves this.
  const moved = useStore((s) => s.panes.map((p) => `${p.id}${p.running}${p.activity}`).join());
  const [list, setList] = useState<Process[]>([]);
  const [stopping, setStopping] = useState<number | null>(null);
  const failure = useFailure();

  useEffect(() => {
    let live = true;
    const load = () =>
      api
        .runningProcesses()
        .then((l) => live && setList(l))
        .catch((e) => live && failure.fail(e));
    void load();
    if (!appActive) {
      return () => {
        live = false;
      };
    }
    // guard: allow poll — processes start and end without telling anyone; only while the window is active.
    const t = window.setInterval(load, 10_000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
    // oxlint-disable-next-line react/exhaustive-deps -- `failure` is a new object each render; reading it then is enough
  }, [moved, appActive]);

  const working = useMemo(
    () =>
      panes.filter(
        (p) => p.kind === "agent" && p.running && p.activity === "working" && !isHenchman(p),
      ),
    [panes],
  );
  const keyOf = (taskId: string) => {
    if (taskId === CHAT_TASK_ID) return "Chat";
    const t = tasks.find((t) => t.id === taskId);
    return t ? (t.issue_key ?? t.name) : "";
  };
  const since = (iso: string) =>
    ran(Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 1000)));

  async function stop(p: Process) {
    setStopping(p.pid);
    failure.clear();
    try {
      setList(await api.stopProcess(p.pid));
    } catch (e) {
      failure.fail(e);
    } finally {
      setStopping(null);
    }
  }

  if (working.length === 0 && list.length === 0 && !failure.error) return null;
  return (
    <section>
      <h2 className="lair-h">At work</h2>
      {working.map((p) => (
        <button
          key={p.id}
          className="lair-work"
          title="Open it"
          onClick={() => goTo(`pane:${p.task_id}:${p.id}`)}
        >
          <span className="ui-live-dot" />
          <span className="w">
            <span className="k">{keyOf(p.task_id)}</span>
            {paneName(p)}
          </span>
          <span className="dur">{since(p.activity_since)}</span>
        </button>
      ))}
      {list.map((p) => (
        <div key={p.pid} className="lair-work proc">
          <span />
          <button
            className="w"
            title={`${p.command}\nStarted in ${p.owner}: open that pane`}
            onClick={() => goTo(`pane:${p.task_id}:${p.pane_id}`)}
          >
            {shortCommand(p.command)}
            {p.ports.length > 0 && ` · :${p.ports.join(", :")}`}
          </button>
          <span className="dur">{ran(p.secs)}</span>
          <button
            className="ui-link side"
            disabled={stopping === p.pid}
            title="SIGTERM, then SIGKILL after 2 seconds"
            onClick={() => void stop(p)}
          >
            {stopping === p.pid ? "Stopping…" : "Stop"}
          </button>
        </div>
      ))}
      <ErrorMessage>{failure.error}</ErrorMessage>
    </section>
  );
}
