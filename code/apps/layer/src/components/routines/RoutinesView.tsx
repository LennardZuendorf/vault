import { useEffect, useRef } from "react";
import { api } from "../../lib/api";
import { menuKeys, pressKeys } from "../../lib/press";
import { standing, useRoutines } from "../../lib/routines";
import type { Routine, RoutineRun } from "../../lib/types-routines";
import { PlusIcon } from "../icons";
import { SidebarGrip } from "../sidebar/SidebarGrip";
import { ErrorMessage, useFailure } from "../ui";
import { RoutinePage } from "./RoutinePage";
import { RunConversation } from "./RunConversation";

/**
 * The Routines tab (§29): the routines down the left, the one chosen beside
 * them, or the conversation of one of its runs.
 */
export function RoutinesView() {
  const all = useRoutines((s) => s.routines);
  const open = useRoutines((s) => s.open);
  const run = useRoutines((s) => s.run);
  const show = useRoutines((s) => s.show);
  const failure = useFailure();
  const making = useRef(false);
  const routines = all?.routines ?? [];
  const runs = all?.runs ?? [];
  const routine = routines.find((r) => r.id === open);
  const shownRun = runs.find((r) => r.id === run && r.routine_id === routine?.id);

  // One gone (deleted, or never here) gives way to the first.
  const first = routines[0]?.id;
  const gone = !!all && !routine;
  useEffect(() => {
    if (gone && first) show(first);
  }, [gone, first, show]);

  async function create() {
    if (making.current) return;
    making.current = true;
    failure.clear();
    try {
      const id = await api.routineCreate();
      await useRoutines.getState().refresh();
      show(id);
    } catch (e) {
      failure.fail(e);
    } finally {
      making.current = false;
    }
  }

  if (all && routines.length === 0) {
    return (
      <div className="routines">
        <div className="empty">
          <h2>Routines</h2>
          <p>
            A routine is a prompt an agent runs on a schedule: a brief each morning, a sweep of open
            pull requests every hour, a check of dependencies once a week. Each run is a new chat,
            kept here with what it said.
          </p>
          <p style={{ color: "var(--faint)" }}>
            When Layer was closed at a routine&apos;s time, it runs once when Layer is back.
          </p>
          <div className="row">
            <button className="ui-button ui-button-primary" onClick={() => void create()}>
              New routine
            </button>
          </div>
          <ErrorMessage>{failure.error}</ErrorMessage>
        </div>
      </div>
    );
  }

  return (
    <div className="chat routines">
      <div className="chat-list">
        <div className="chat-list-head">
          Routines
          <div className="spacer" />
          <button
            className="ui-button ui-icon-button btn-add"
            title="New routine"
            onClick={() => void create()}
          >
            <PlusIcon />
          </button>
        </div>
        <ErrorMessage>{failure.error}</ErrorMessage>
        <div className="chat-list-body">
          {routines.map((r) => (
            <RoutineItem
              key={r.id}
              routine={r}
              last={runs.find((x) => x.routine_id === r.id)}
              active={r.id === routine?.id}
              onSelect={show}
            />
          ))}
        </div>
      </div>
      <SidebarGrip side="chats" />
      <div className="chat-main">
        {routine && shownRun ? (
          <RunConversation routine={routine} run={shownRun} />
        ) : routine ? (
          <RoutinePage
            key={routine.id}
            routine={routine}
            runs={runs.filter((r) => r.routine_id === routine.id)}
          />
        ) : null}
      </div>
    </div>
  );
}

/** A run at work is a live dot, one asking amber; a routine that is off is faint. */
function dotOf(r: Routine, last: RoutineRun | undefined): string {
  if (last?.state === "working") return "live";
  if (last?.state === "asking") return "idle";
  return r.on ? "" : "off";
}

function RoutineItem({
  routine: r,
  last,
  active,
  onSelect,
}: {
  routine: Routine;
  last: RoutineRun | undefined;
  active: boolean;
  onSelect: (id: string) => void;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      aria-label={r.name}
      className={`chat-item${active ? " active" : ""}`}
      onClick={() => onSelect(r.id)}
      onKeyDown={menuKeys(pressKeys(() => onSelect(r.id)))}
    >
      <span className={`dot ${dotOf(r, last)}`} />
      <span className="chat-item-text">
        <span className="chat-item-title" title={r.name}>
          {r.name}
        </span>
        <span className="chat-item-sub">{standing(r, last)}</span>
      </span>
    </div>
  );
}
