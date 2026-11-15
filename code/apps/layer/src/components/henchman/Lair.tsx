import { useMemo, useState } from "react";
import { paneName, paneState } from "../../lib/derive";
import { goTo } from "../../lib/goto";
import { useOrders } from "../../lib/henchman-orders";
import { LAIR_GROUPS, lair, type LairGroup, type LairRow } from "../../lib/lair";
import { read, write } from "../../lib/persist";
import type { PaneInfo, TaskView } from "../../lib/types";
import { useStore } from "../../store";
import { FinishTask } from "../FinishTask";
import { ChevronIcon } from "../icons";
import { Orders } from "./Orders";
import { Running } from "./Running";

type Fold = LairGroup | "chats" | "running" | "orders";

/**
 * The lair (HEN-5): every task by what it needs, current without being
 * asked. Needs you and To finish are rows with their one button; the rest
 * are folded to counts and opened with a click.
 */
export function LairColumn() {
  const tasks = useStore((s) => s.tasks);
  const panes = useStore((s) => s.panes);
  const prs = useStore((s) => s.prs);
  const select = useStore((s) => s.select);
  const l = useMemo(() => lair(tasks, panes, prs), [tasks, panes, prs]);
  const [opened, setOpened] = useState(() => read<Partial<Record<Fold, boolean>>>("lairOpen", {}));
  const [finishing, setFinishing] = useState<TaskView | null>(null);
  const [processes, setProcesses] = useState(0);
  const standing = useOrders((s) =>
    s.orders ? s.orders.orders.length + s.orders.left_alone.length : 0,
  );

  const isOpen = (id: Fold, byDefault: boolean) => opened[id] ?? byDefault;
  const toggle = (id: Fold, byDefault: boolean) => {
    const next = { ...opened, [id]: !isOpen(id, byDefault) };
    setOpened(next);
    write("lairOpen", next);
  };
  const asking = l.chats.filter((p) => p.running && p.activity === "asking");

  return (
    <aside className="lair" aria-label="The lair">
      {LAIR_GROUPS.map((g) => {
        const rows = l.groups[g.id];
        const chats = g.id === "needs_you" ? asking : [];
        const n = rows.length + chats.length;
        const open = isOpen(g.id, g.open);
        return (
          <section key={g.id}>
            <Head label={g.label} n={n} open={open} onToggle={() => toggle(g.id, g.open)} />
            {open && (
              <>
                {chats.map((p) => (
                  <ChatRow key={p.id} pane={p} why="asks you" button />
                ))}
                {rows.map((r, i) => (
                  <TaskRow
                    key={r.task.id}
                    row={r}
                    primary={g.id === "needs_you" && i === 0 && chats.length === 0}
                    act={g.id === "to_finish" ? "Finish" : g.id === "needs_you" ? "Open" : null}
                    onOpen={() => select(r.task.id)}
                    onAct={() => (g.id === "to_finish" ? setFinishing(r.task) : select(r.task.id))}
                  />
                ))}
                {n === 0 && <div className="lair-none">None</div>}
              </>
            )}
          </section>
        );
      })}
      <section>
        <Head
          label="Chats"
          n={l.chats.length}
          open={isOpen("chats", false)}
          onToggle={() => toggle("chats", false)}
        />
        {isOpen("chats", false) &&
          (l.chats.length ? (
            l.chats.map((p) => <ChatRow key={p.id} pane={p} why={paneState(p).label} />)
          ) : (
            <div className="lair-none">None open</div>
          ))}
      </section>
      <section>
        <Head
          label="Running"
          n={processes}
          open={isOpen("running", false)}
          onToggle={() => toggle("running", false)}
        />
        <Running shown={isOpen("running", false)} onCount={setProcesses} />
      </section>
      <section>
        <Head
          label="Standing orders"
          n={standing}
          open={isOpen("orders", false)}
          onToggle={() => toggle("orders", false)}
        />
        <Orders shown={isOpen("orders", false)} />
      </section>
      {finishing && <FinishTask task={finishing} onClose={() => setFinishing(null)} />}
    </aside>
  );
}

function Head({
  label,
  n,
  open,
  onToggle,
}: {
  label: string;
  n: number;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button className="ui-disclosure lair-head" aria-expanded={open} onClick={onToggle}>
      <ChevronIcon size={13} />
      <span>{label}</span>
      <span className="n">{n}</span>
    </button>
  );
}

const TONE: Record<string, string> = {
  "changes asked for": "lw-danger",
  "checks failed": "lw-danger",
  "its agent is out of budget": "lw-danger",
  "its agent asks": "lw-warn",
  "its agent finished: your turn": "lw-warn",
};

function TaskRow({
  row,
  primary,
  act,
  onOpen,
  onAct,
}: {
  row: LairRow;
  primary: boolean;
  act: string | null;
  onOpen: () => void;
  onAct: () => void;
}) {
  const { task } = row;
  return (
    <div className="lair-row">
      <button className="lt" title={`Open ${task.issue_key ?? task.name} in Work`} onClick={onOpen}>
        {task.issue_key && <span className="key">{task.issue_key}</span>}
        {task.issue_key ? task.name.replace(`${task.issue_key} `, "") : task.name}
      </button>
      <div className={`lw ${TONE[row.why] ?? ""}`}>{row.why}</div>
      {act && (
        <button className={`ui-button${primary ? " ui-button-primary" : ""}`} onClick={onAct}>
          {act}
        </button>
      )}
    </div>
  );
}

function ChatRow({ pane, why, button }: { pane: PaneInfo; why: string; button?: boolean }) {
  const open = () => goTo(`pane:chat:${pane.id}`);
  return (
    <div className="lair-row">
      <button className="lt" title="Open it in Chat" onClick={open}>
        <span className="key">Chat</span>
        {paneName(pane)}
      </button>
      <div className={`lw${button ? " lw-warn" : ""}`}>{why}</div>
      {button && (
        <button className="ui-button" onClick={open}>
          Open
        </button>
      )}
    </div>
  );
}
