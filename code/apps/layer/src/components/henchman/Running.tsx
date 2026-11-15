import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import { goTo } from "../../lib/goto";
import { ran, shortCommand } from "../../lib/lair";
import type { Process } from "../../lib/types-henchman";
import { useStore } from "../../store";
import { ErrorMessage, useFailure } from "../ui";

/**
 * What the panes started that still runs (HEN-6): dev servers, builds,
 * watchers. Read when the tab opens and when a pane's state moves, and
 * every 10 seconds while shown, since a process starts and ends without
 * telling anyone.
 */
export function Running({ shown, onCount }: { shown: boolean; onCount: (n: number) => void }) {
  const appActive = useStore((s) => s.appActive);
  // What a pane does, not its output: a server started or stopped moves this.
  const moved = useStore((s) => s.panes.map((p) => `${p.id}${p.running}${p.activity}`).join());
  const [list, setList] = useState<Process[] | null>(null);
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
    // guard: allow poll — processes start and end without telling anyone; only while the tab is shown and the window active.
    const t = window.setInterval(load, 10_000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
    // oxlint-disable-next-line react/exhaustive-deps -- `failure` is a new object each render; reading it then is enough
  }, [moved, appActive]);

  useEffect(() => onCount(list?.length ?? 0), [list, onCount]);

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

  if (!shown) return null;
  return (
    <>
      {list?.length === 0 && <div className="lair-none">Nothing the panes started still runs</div>}
      {list?.map((p) => (
        <div key={p.pid} className="lair-row lair-proc">
          <div className="lt" title={p.command}>
            {shortCommand(p.command)}
          </div>
          <div className="lw">
            {p.ports.length > 0 && `:${p.ports.join(", :")} · `}
            {`${ran(p.secs)} · `}
            <button
              className="ui-link"
              title="Open the pane that started it"
              onClick={() => goTo(`pane:${p.task_id}:${p.pane_id}`)}
            >
              {p.owner}
            </button>
          </div>
          <button
            className="ui-button"
            disabled={stopping === p.pid}
            title="SIGTERM, then SIGKILL after 2 seconds"
            onClick={() => void stop(p)}
          >
            {stopping === p.pid ? "Stopping…" : "Stop"}
          </button>
        </div>
      ))}
      <ErrorMessage>{failure.error}</ErrorMessage>
    </>
  );
}
