import { useMemo, useState } from "react";
import { api } from "../../lib/api";
import { goTo } from "../../lib/goto";
import { useLair } from "../../lib/henchman-lair";
import { useOrders } from "../../lib/henchman-orders";
import { waiting, type Waiting } from "../../lib/lair";
import type { TaskView } from "../../lib/types";
import type { HenchmanNeed } from "../../lib/types-henchman";
import { useStore } from "../../store";
import { FinishTask } from "../FinishTask";
import { ErrorMessage, useFailure } from "../ui";
import { AtWork } from "./AtWork";
import { OrdersDialog } from "./Orders";

/** Lines of "Done for you" shown before "N more". */
const SHOWN = 5;

/**
 * The lair (HEN-5): beside the Henchman, only what the user can do, what
 * the Henchman did for them, and what is at work now. Everything else is
 * in Work, one link away.
 */
export function LairColumn() {
  const tasks = useStore((s) => s.tasks);
  const panes = useStore((s) => s.panes);
  const prs = useStore((s) => s.prs);
  const select = useStore((s) => s.select);
  const setView = useStore((s) => s.setView);
  const lair = useLair((s) => s.lair);
  const setLair = useLair((s) => s.set);
  const standing = useOrders((s) =>
    s.orders ? s.orders.orders.length + s.orders.left_alone.length : 0,
  );
  const waits = useMemo(() => waiting(tasks, panes, prs), [tasks, panes, prs]);
  const [finishing, setFinishing] = useState<TaskView | null>(null);
  const [orders, setOrders] = useState(false);
  const [more, setMore] = useState(false);
  const failure = useFailure();

  const needs = useMemo(() => [...(lair?.needs ?? [])].reverse(), [lair]);
  const did = lair?.did ?? [];
  const count = needs.length + waits.length;
  const named = new Set(waits.flatMap((w) => (w.task ? [w.task.id] : [])));
  const others = tasks.filter((t) => !named.has(t.id)).length;

  async function done(need: HenchmanNeed) {
    failure.clear();
    try {
      setLair(await api.henchmanNeedDone(need.id));
    } catch (e) {
      failure.fail(e);
    }
  }

  function openWait(w: Waiting) {
    if (w.finish && w.task) setFinishing(w.task);
    else if (w.pane) goTo(`pane:${w.pane.task_id}:${w.pane.id}`);
    else if (w.task) select(w.task.id);
  }

  return (
    <aside className="lair" aria-label="The lair">
      <section>
        <h2 className="lair-h">Needs you {count > 0 && <span className="n">{count}</span>}</h2>
        {count === 0 && <div className="lair-calm">Nothing needs you.</div>}
        {needs.map((n, i) => (
          <NeedRow
            key={n.id}
            first={i === 0}
            k={n.key}
            at={n.at}
            text={n.text}
            act={n.taskId ? "Open" : null}
            onAct={() => n.taskId && select(n.taskId)}
            onDone={() => void done(n)}
          />
        ))}
        {waits.map((w, i) => (
          <NeedRow
            key={w.id}
            first={needs.length === 0 && i === 0}
            k={w.key}
            text={w.why}
            title={w.task?.name}
            act={w.finish ? "Finish" : "Open"}
            onAct={() => openWait(w)}
          />
        ))}
        <ErrorMessage>{failure.error}</ErrorMessage>
      </section>
      {did.length > 0 && (
        <section>
          <h2 className="lair-h">Done for you</h2>
          <div className="lair-dids">
            {(more ? did : did.slice(0, SHOWN)).map((d) => (
              <div key={`${d.at}${d.what}`} className="lair-did">
                <span className="at">{when(d.at)}</span>
                {d.taskId && d.key ? (
                  <button
                    className="k"
                    title={`Open ${d.key} in Work`}
                    onClick={() => select(d.taskId!)}
                  >
                    {d.key}
                  </button>
                ) : (
                  <span className="k">{d.key}</span>
                )}
                <span className="w" title={d.what}>
                  {d.what}
                </span>
              </div>
            ))}
          </div>
          {did.length > SHOWN && (
            <button className="ui-link lair-more" onClick={() => setMore(!more)}>
              {more ? "Fewer" : `${did.length - SHOWN} more`}
            </button>
          )}
        </section>
      )}
      <AtWork />
      <div className="lair-foot">
        {others > 0 && (
          <button className="ui-link" onClick={() => setView("work")}>
            {others} {count > 0 ? "others" : others === 1 ? "task" : "tasks"} in Work
          </button>
        )}
        <button className="ui-link" onClick={() => setOrders(true)}>
          {standing > 0
            ? `${standing} standing order${standing === 1 ? "" : "s"}`
            : "Standing orders"}
        </button>
      </div>
      {finishing && <FinishTask task={finishing} onClose={() => setFinishing(null)} />}
      {orders && <OrdersDialog onClose={() => setOrders(false)} />}
    </aside>
  );
}

/**
 * One thing that needs the user. The first has its action as the primary
 * button; the rest show theirs on hover, where the time was, so a row
 * never changes height under the pointer.
 */
function NeedRow({
  first,
  k,
  at,
  text,
  title,
  act,
  onAct,
  onDone,
}: {
  first: boolean;
  k: string | null;
  at?: string;
  text: string;
  title?: string;
  act: string | null;
  onAct: () => void;
  onDone?: () => void;
}) {
  const acts = (
    <>
      {act && (
        <button className={first ? "ui-button ui-button-primary" : "ui-link"} onClick={onAct}>
          {act}
        </button>
      )}
      {onDone && (
        <button className="ui-link" title="Done, or not yours: it leaves the lair" onClick={onDone}>
          Done
        </button>
      )}
    </>
  );
  return (
    <div className={`lair-need${first ? " first" : ""}`} title={title}>
      <span className="k">{k}</span>
      {!first && <span className="side">{acts}</span>}
      {at && <span className="when">{when(at)}</span>}
      <div className="t">{text}</div>
      {first && <div className="acts">{acts}</div>}
    </div>
  );
}

/** "10:42" today, "Nov 14" before. */
function when(iso: string): string {
  const d = new Date(iso);
  return d.toDateString() === new Date().toDateString()
    ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
}
