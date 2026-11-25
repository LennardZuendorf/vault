import { useState } from "react";
import { api } from "../../lib/api";
import { day, outcome, schedule, useRoutines, WEEKDAYS } from "../../lib/routines";
import type { Every, Routine, RoutinePatch, RoutineRun } from "../../lib/types-routines";
import { useStore } from "../../store";
import { AgentAndModel } from "../AgentAndModel";
import { BusyLabel, Confirm, ErrorMessage, Field, Switch, useFailure } from "../ui";

const EVERY: [Every, string][] = [
  ["hourly", "Hourly"],
  ["daily", "Daily"],
  ["weekdays", "Weekdays"],
  ["weekly", "Weekly"],
];

/** The efforts offered, as for the Henchman (HEN-15). */
const EFFORTS = [
  ["low", "Low"],
  ["medium", "Medium"],
  ["high", "High"],
] as const;

/**
 * One routine (ROUT-2, ROUT-8): what it asks, when, with whom, and its
 * runs. Each field saves when it changes or loses focus; nothing waits for
 * a Save button. Keyed by the routine, so a draft never leaks to another.
 */
export function RoutinePage({ routine: r, runs }: { routine: Routine; runs: RoutineRun[] }) {
  const agents = useStore((s) => s.agents);
  const refreshPanes = useStore((s) => s.refreshPanes);
  const showRun = useRoutines((s) => s.showRun);
  const failure = useFailure();
  const [name, setName] = useState(r.name);
  const [prompt, setPrompt] = useState(r.prompt);
  const [at, setAt] = useState(r.at);
  const [starting, setStarting] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const conversational = agents.filter((a) => a.installed && a.acp);
  const agentName = agents.find((a) => a.id === r.agent.agent_id)?.name ?? r.agent.agent_id;
  const live = runs.find((x) => x.state === "working" || x.state === "asking");
  const hourly = r.every === "hourly";

  async function save(patch: RoutinePatch, undo?: () => void) {
    failure.clear();
    try {
      await api.routineUpdate(r.id, patch);
    } catch (e) {
      undo?.();
      failure.fail(e);
    }
  }

  // An hourly routine reads only the minutes, so it is asked for only them.
  const time = hourly ? (at.split(":")[1] ?? "00") : at;
  function saveTime(value: string) {
    const next = hourly ? `${r.at.split(":")[0] ?? "00"}:${value.padStart(2, "0")}` : value;
    if (next === r.at) return;
    setAt(next);
    void save({ at: next }, () => setAt(r.at));
  }

  async function runNow() {
    if (starting) return;
    setStarting(true);
    failure.clear();
    try {
      const run = await api.routineRunNow(r.id);
      await Promise.all([useRoutines.getState().refresh(), refreshPanes()]);
      // Watch it work; one that could not start stays on the list, saying why.
      if (run.state !== "failed") showRun(run.id);
    } catch (e) {
      failure.fail(e);
    } finally {
      setStarting(false);
    }
  }

  async function open(run: RoutineRun) {
    failure.clear();
    if (!run.room) return;
    try {
      await api.routineOpenRun(run.id);
      await Promise.all([useRoutines.getState().refresh(), refreshPanes()]);
      showRun(run.id);
    } catch (e) {
      failure.fail(e);
    }
  }

  const next = r.next ? `next run ${day(r.next)}` : r.on ? "" : "off";
  return (
    <div className="routine-page">
      <div className="routine-head">
        <input
          className="routine-name"
          aria-label="Name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            if (name.trim() === r.name) return;
            void save({ name }, () => setName(r.name));
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") {
              setName(r.name);
              e.currentTarget.blur();
            }
          }}
        />
        <div className="routine-on">
          <Switch label="On" checked={r.on} onChange={(on) => void save({ on })} />
        </div>
        <button
          className="ui-button ui-button-primary"
          disabled={starting || !!live || !r.prompt.trim()}
          title={
            live
              ? "Its run is still at work"
              : !r.prompt.trim()
                ? "Write what it asks first"
                : undefined
          }
          onClick={() => void runNow()}
        >
          <BusyLabel busy={starting} idle="Run now" working="Starting…" />
        </button>
      </div>
      <div className="routine-sub">
        {[schedule(r), agentName, next].filter(Boolean).join(" · ")}
      </div>
      <ErrorMessage>{failure.error}</ErrorMessage>

      <Field label="What it asks" hint="The first message of each run's chat.">
        <textarea
          className="ui-field routine-prompt"
          value={prompt}
          placeholder="Read work_overview. Tell me what needs me today, at most five lines, the most pressing first."
          onChange={(e) => setPrompt(e.target.value)}
          onBlur={() => {
            if (prompt === r.prompt) return;
            void save({ prompt }, () => setPrompt(r.prompt));
          }}
        />
      </Field>

      <Field
        label="When"
        hint="On this Mac's clock. If Layer is closed then, it runs once when Layer is back."
      >
        <div className="routine-when">
          <div className="ui-segmented">
            {EVERY.map(([value, label]) => (
              <button
                key={value}
                className="ui-segment"
                aria-pressed={r.every === value}
                onClick={() => void save({ every: value })}
              >
                {label}
              </button>
            ))}
          </div>
          {r.every === "weekly" && (
            <>
              <span className="muted">on</span>
              <select
                className="ui-field"
                aria-label="Day"
                value={r.weekday}
                onChange={(e) => void save({ weekday: Number(e.target.value) })}
              >
                {WEEKDAYS.map((d, i) => (
                  <option key={d} value={i}>
                    {d}
                  </option>
                ))}
              </select>
            </>
          )}
          <span className="muted">{hourly ? "at minute" : "at"}</span>
          <input
            key={`${r.every}-${r.at}`}
            className="ui-field routine-time"
            aria-label={hourly ? "Minute" : "Time"}
            type={hourly ? "number" : "time"}
            min={hourly ? 0 : undefined}
            max={hourly ? 59 : undefined}
            defaultValue={time}
            onBlur={(e) => saveTime(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
          />
        </div>
      </Field>

      <AgentAndModel
        agentId={r.agent.agent_id}
        model={r.agent.model}
        conversational={conversational}
        agentHint="Any agent that runs as a conversation."
        modelHint="Each run opens on it."
        onChange={(agentId, model) =>
          void save(agentId === r.agent.agent_id ? { model } : { agent_id: agentId })
        }
      >
        <Field label="Effort" hint="Where its model has one.">
          <select
            className="ui-field"
            value={r.agent.effort}
            onChange={(e) => void save({ effort: e.target.value })}
          >
            <option value="">{agentName} decides</option>
            {EFFORTS.map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </Field>
      </AgentAndModel>

      <div className="routine-runs-head">Runs</div>
      {runs.length === 0 ? (
        <div className="muted routine-none">
          {!r.prompt.trim()
            ? "None yet. Write what it asks, and it runs on its schedule."
            : r.on && r.next
              ? `Its first run is ${day(r.next)}, or Run now.`
              : "None yet. Turn it on, or Run now."}
        </div>
      ) : (
        <div className="routine-runs">
          {runs.map((run) => (
            <RunRow key={run.id} run={run} onOpen={() => void open(run)} />
          ))}
        </div>
      )}

      <div className="routine-foot">
        <button className="ui-button" onClick={() => setDeleting(true)}>
          Delete routine…
        </button>
      </div>

      {deleting && (
        <Confirm
          title={`Delete ${r.name}?`}
          confirmLabel="Delete routine"
          busyLabel="Deleting…"
          body={
            <>
              Its runs go with it{live ? ", and the one at work is stopped" : ""}. This can&apos;t
              be undone.
            </>
          }
          onCancel={() => setDeleting(false)}
          onConfirm={async () => {
            await api.routineDelete(r.id);
            await Promise.all([useRoutines.getState().refresh(), refreshPanes()]);
          }}
        />
      )}
    </div>
  );
}

const DOT: Record<RoutineRun["state"], string> = {
  working: "live",
  asking: "idle",
  done: "",
  stopped: "",
  skipped: "",
  failed: "gone",
};

function RunRow({ run, onOpen }: { run: RoutineRun; onOpen: () => void }) {
  const said =
    run.said ||
    (run.state === "working" ? "Working…" : run.state === "asking" ? "Asking you…" : "");
  const inside = (
    <>
      <span className={`dot ${DOT[run.state]}`} />
      <span className="when">{run.due ? day(run.due) : `Run now, ${day(run.started)}`}</span>
      <span className="what" title={run.said}>
        {said}
      </span>
      <span className={`how ${run.state}`}>{outcome(run)}</span>
    </>
  );
  // One that never started has no conversation to open.
  return run.room ? (
    <button type="button" className="routine-run opens" onClick={onOpen}>
      {inside}
    </button>
  ) : (
    <div className="routine-run">{inside}</div>
  );
}
