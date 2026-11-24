import { useEffect, useState } from "react";
import { requirementsPath, useSpecs } from "../lib/specs";
import type { SpecKind, SpecPart, TaskView } from "../lib/types";
import { useStore } from "../store";
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
  const [path, setPath] = useState<string | null>(null);
  const [kind, setKind] = useState<SpecKind>("feature");
  const [committing, setCommitting] = useState(false);

  const loaded = state?.loaded ?? false;
  useEffect(() => {
    if (!loaded) void load(task.id).catch(fail);
  }, [task.id, loaded, load, fail]);

  const spec = state?.spec ?? null;
  if (!loaded || !spec) return null;

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
      {spec.unreadable.length > 0 && (
        <p className="spec-unreadable" role="status">
          {spec.unreadable.join(", ")} {spec.unreadable.length === 1 ? "is" : "are"} not UTF-8 text,
          so {spec.unreadable.length === 1 ? "it is" : "they are"} not shown here and Commit spec
          leaves {spec.unreadable.length === 1 ? "it" : "them"} out.
        </p>
      )}
      {spec.files.length === 0 && current === null ? (
        <div className="empty">
          <h2>No spec yet</h2>
          <p>
            The spec is the files in <code>{spec.dir}</code>. An agent writes them there, or you
            can.
          </p>
        </div>
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
