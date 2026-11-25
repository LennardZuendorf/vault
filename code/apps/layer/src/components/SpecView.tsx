import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { requirementsPath, useSpecs } from "../lib/specs";
import type { SpecKind, SpecPart, TaskView } from "../lib/types";
import { useStore } from "../store";
import { AgentTargetFields, useAgentTarget } from "./AgentTarget";
import { SpecEditor } from "./spec/SpecEditor";
import { SpecQuestions } from "./spec/SpecQuestions";
import { SpecSide } from "./spec/SpecSide";
import { SpecTree } from "./spec/SpecTree";
import { BusyLabel, ErrorMessage, Spinner, useFailure } from "./ui";

const DRAFTS: { part: SpecPart; label: string }[] = [
  { part: "requirements", label: "Draft requirements" },
  { part: "design", label: "Draft design" },
  { part: "tasks", label: "Draft tasks" },
];

/**
 * A task's spec (§19): the files its toolset keeps in `<task>/spec/` as a
 * tree, each opened in an editor and saved as a plain file write, the way an
 * agent writes it, and followed live while an agent does. Beside them: how far
 * each repository is through the steps, and Commit spec. Kiro's extras (Draft,
 * Apply answers, Check against spec) show only for a toolset that has them.
 * It is read every time it is shown: the folder is followed only while it is,
 * and an agent may have written since.
 */
export function SpecView({ task }: { task: TaskView }) {
  const state = useSpecs((s) => s.byTask[task.id]);
  const install = useSpecs((s) => s.install[task.id]);
  const load = useSpecs((s) => s.load);
  const draft = useSpecs((s) => s.draft);
  const undo = useSpecs((s) => s.undo);
  const commitSpec = useSpecs((s) => s.commitSpec);
  const retryInstall = useSpecs((s) => s.retryInstall);
  const fail = useStore((s) => s.fail);
  const actions = useFailure();
  // A read that fails says so here, with Try again, not in the window's corner.
  const reading = useFailure();
  const { clear: clearReading, fail: failReading } = reading;
  const [reads, setReads] = useState(0);
  const [path, setPath] = useState<string | null>(null);
  const [kind, setKind] = useState<SpecKind>("feature");
  const [committing, setCommitting] = useState(false);

  useEffect(() => {
    clearReading();
    void load(task.id).catch(failReading);
  }, [task.id, load, reads, clearReading, failReading]);
  const readAgain = () => setReads((n) => n + 1);

  const loaded = state?.loaded ?? false;
  const spec = state?.spec ?? null;
  if (!loaded || !spec) {
    if (!reading.error) return null;
    return (
      <div className="empty">
        <h2>The spec could not be read</h2>
        <ErrorMessage>{reading.error}</ErrorMessage>
        <button className="ui-button ui-button-primary" onClick={readAgain}>
          Try again
        </button>
      </div>
    );
  }

  const edits = state?.edits ?? {};
  const unsaved = Object.keys(edits).length > 0;
  const drafting = !!state?.drafting;
  const extras = spec.toolset.extras;
  const current =
    path && (spec.files.some((f) => f.path === path) || edits[path] !== undefined)
      ? path
      : (spec.files[0]?.path ?? Object.keys(edits)[0] ?? null);
  const shown = install ?? spec.install;

  async function run(what: () => Promise<unknown>) {
    actions.clear();
    try {
      await what();
    } catch (e) {
      actions.fail(e);
    }
  }

  async function commit() {
    setCommitting(true);
    await run(() => commitSpec(task.id));
    setCommitting(false);
  }

  return (
    <div className="spec">
      <div className="spec-bar">
        <b>{spec.toolset.name}</b>
        {spec.toolset.version && <span className="muted">{spec.toolset.version}</span>}
        {shown?.running && (
          <>
            <Spinner />
            <span className="muted">Installing…</span>
          </>
        )}
        {shown && !shown.running && !shown.ok && (
          <>
            <span className="spec-failed">
              Install failed: {shown.error ?? "it gave no reason"}.
            </span>
            <button className="ui-button" onClick={() => void retryInstall(task.id).catch(fail)}>
              Retry
            </button>
          </>
        )}
        <span className="spacer" />
        <button
          className="ui-button"
          title={
            unsaved
              ? "Save or discard your edits first: Commit spec puts what is saved into each repository."
              : "Put the spec into each repository, as its setting says, and commit it there"
          }
          disabled={drafting || committing || unsaved}
          onClick={() => void commit()}
        >
          <BusyLabel busy={committing} idle="Commit spec" working="Committing…" />
        </button>
      </div>
      {extras.includes("draft") && (
        <div className="spec-bar">
          {spec.files.length === 0 && (
            <select
              className="ui-field"
              aria-label="What the spec is for"
              value={kind}
              onChange={(e) => setKind(e.target.value as SpecKind)}
            >
              <option value="feature">A feature</option>
              <option value="bugfix">A bug</option>
            </select>
          )}
          {DRAFTS.map((d) => (
            <button
              key={d.part}
              className="ui-button"
              disabled={drafting || unsaved}
              title={
                unsaved
                  ? "Save or discard your edits first: a draft is written into the files, and unsaved text would be lost."
                  : undefined
              }
              onClick={() => void run(() => draft(task.id, d.part, kind))}
            >
              {d.label}
            </button>
          ))}
          {drafting && <Spinner />}
          {state?.last && !drafting && (
            <button className="ui-button" onClick={() => void run(() => undo(task.id))}>
              Undo
            </button>
          )}
        </div>
      )}
      <ErrorMessage>{actions.error}</ErrorMessage>
      {reading.error && (
        <div className="spec-bar">
          <span className="spec-failed">
            The spec was not read again, so what is shown may be out of date. {reading.error}
          </span>
          <button className="ui-button" onClick={readAgain}>
            Try again
          </button>
        </div>
      )}
      {spec.unreadable.length > 0 && (
        <p className="spec-unreadable" role="status">
          {spec.unreadable.join(", ")} {spec.unreadable.length === 1 ? "is" : "are"} not UTF-8 text,
          so {spec.unreadable.length === 1 ? "it is" : "they are"} not shown here and Commit spec
          leaves {spec.unreadable.length === 1 ? "it" : "them"} out.
        </p>
      )}
      {spec.files.length === 0 && current === null ? (
        <NoSpec task={task} dir={spec.dir} ask={!task.epic} />
      ) : (
        <div className="spec-body">
          <SpecTree
            paths={spec.files.map((f) => f.path)}
            current={current}
            edits={edits}
            onPick={setPath}
          />
          {current !== null && (
            <SpecEditor
              key={current}
              task={task}
              file={spec.files.find((f) => f.path === current)}
              path={current}
              readOnly={drafting}
            />
          )}
          <SpecSide task={task} spec={spec}>
            {extras.includes("questions") && requirementsPath(spec) === current && (
              <SpecQuestions task={task} path={current} />
            )}
          </SpecSide>
        </div>
      )}
    </div>
  );
}

/**
 * An empty spec: where its files go, and Ask an agent to write it (SPEC-8),
 * sent to an agent already running in the task, or to one started with it.
 * The agent is told what Spec first tells it (SPEC-38): write the spec, then
 * stop for your go. Not on an epic's page, which has no agents of its own to
 * start: its chat is beside it.
 */
function NoSpec({ task, dir, ask: offered }: { task: TaskView; dir: string; ask: boolean }) {
  const at = useAgentTarget(task);
  const asking = useFailure();
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<string | null>(null);

  async function ask() {
    setBusy(true);
    asking.clear();
    try {
      const who = await at.send((paneId) => api.askForSpec(task.id, paneId));
      setSent(who);
    } catch (e) {
      asking.fail(e);
    }
    setBusy(false);
  }

  return (
    <div className="empty">
      <h2>No spec yet</h2>
      <p>
        The spec is the files in <code>{dir}</code>. An agent writes them there, or you can.
      </p>
      {offered && (
        <div className="row">
          <AgentTargetFields task={task} at={at} disabled={busy} />
          <button
            className="ui-button ui-button-primary"
            disabled={busy || at.stuck}
            title={at.stuck ? "No agent is installed to ask." : undefined}
            onClick={() => void ask()}
          >
            <BusyLabel busy={busy} idle="Ask an agent to write it" working="Asking…" />
          </button>
        </div>
      )}
      {sent && <p role="status">Asked {sent}. The spec shows here as it is written.</p>}
      <ErrorMessage>{asking.error}</ErrorMessage>
    </div>
  );
}
