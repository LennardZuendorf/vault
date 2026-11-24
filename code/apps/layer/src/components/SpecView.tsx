import { useEffect, useState } from "react";
import { editorText, requirementsPath, useSpecs } from "../lib/specs";
import type { RepoResult, SpecKind, SpecPart, TaskView } from "../lib/types";
import { useStore } from "../store";
import { SpecQuestions } from "./spec/SpecQuestions";
import { SpecSide } from "./spec/SpecSide";
import { ErrorMessage, Spinner, useFailure } from "./ui";

const DRAFTS: { part: SpecPart; label: string }[] = [
  { part: "requirements", label: "Draft requirements" },
  { part: "design", label: "Draft design" },
  { part: "tasks", label: "Draft tasks" },
];

/**
 * A task's spec (§19): the files its toolset keeps in `<task>/spec/`, each
 * opened in an editor and saved as a plain file write, the way an agent
 * writes it. Kiro's extras (Draft, Apply answers, Check against spec) show
 * only for a toolset that has them.
 */
export function SpecView({ task }: { task: TaskView }) {
  const state = useSpecs((s) => s.byTask[task.id]);
  const install = useSpecs((s) => s.install[task.id]);
  const load = useSpecs((s) => s.load);
  const edit = useSpecs((s) => s.edit);
  const save = useSpecs((s) => s.save);
  const discard = useSpecs((s) => s.discard);
  const draft = useSpecs((s) => s.draft);
  const undo = useSpecs((s) => s.undo);
  const commitSpec = useSpecs((s) => s.commitSpec);
  const retryInstall = useSpecs((s) => s.retryInstall);
  const pending = useSpecs((s) => s.pending);
  const setPending = useSpecs((s) => s.setPending);
  const fail = useStore((s) => s.fail);
  const actions = useFailure();
  const [path, setPath] = useState<string | null>(null);
  const [kind, setKind] = useState<SpecKind>("feature");
  const [committed, setCommitted] = useState<RepoResult[] | null>(null);

  const loaded = state?.loaded ?? false;
  useEffect(() => {
    if (!loaded) void load(task.id).catch(fail);
  }, [task.id, loaded, load, fail]);
  // The agent Start work picked is no longer started from this tab.
  useEffect(() => {
    if (pending?.taskId === task.id) setPending(null);
  }, [pending, task.id, setPending]);

  const spec = state?.spec ?? null;
  if (!loaded || !spec) return null;

  const edits = state?.edits ?? {};
  const drafting = !!state?.drafting;
  const extras = spec.toolset.extras;
  const current =
    path && (spec.files.some((f) => f.path === path) || edits[path] !== undefined)
      ? path
      : (spec.files[0]?.path ?? null);
  const dirty = current !== null && edits[current] !== undefined;
  const install_ = install ?? spec.install;

  async function run(what: () => Promise<unknown>) {
    actions.clear();
    try {
      await what();
    } catch (e) {
      actions.fail(e);
    }
  }

  return (
    <div className="spec">
      <div className="spec-bar">
        <b>{spec.toolset.name}</b>
        {spec.toolset.version && <span className="muted">{spec.toolset.version}</span>}
        {install_?.running && (
          <>
            <Spinner />
            <span className="muted">Installing…</span>
          </>
        )}
        {install_ && !install_.running && !install_.ok && (
          <>
            <span className="spec-stale">Install failed: {install_.error}</span>
            <button className="ui-button" onClick={() => void retryInstall(task.id).catch(fail)}>
              Retry
            </button>
          </>
        )}
        <span className="spacer" />
        <button
          className="ui-button"
          title="Put the spec into each repository, as its setting says, and commit it there"
          disabled={drafting}
          onClick={() => void run(async () => setCommitted(await commitSpec(task.id)))}
        >
          Commit spec
        </button>
      </div>
      {committed && (
        <div className="muted">
          {committed.map((r) => `${r.repo}: ${r.detail}`).join(" · ") || "No repositories."}
        </div>
      )}
      {extras.includes("draft") && (
        <div className="spec-bar">
          {spec.files.length === 0 && (
            <select
              className="ui-field"
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
              disabled={drafting}
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
          <div className="spec-repos">
            {spec.files.map((f) => (
              <button
                key={f.path}
                className={`spec-repo${f.path === current ? " active" : ""}`}
                onClick={() => setPath(f.path)}
              >
                {f.path}
                {edits[f.path] !== undefined && <span className="muted"> · unsaved</span>}
              </button>
            ))}
          </div>
          {current !== null && (
            <div>
              <div className="spec-bar">
                <button
                  className="ui-button ui-button-primary"
                  disabled={!dirty || drafting}
                  onClick={() => void run(() => save(task.id, current))}
                >
                  Save
                </button>
                {dirty && !drafting && (
                  <button className="ui-button" onClick={() => discard(task.id, current)}>
                    Discard changes
                  </button>
                )}
              </div>
              <textarea
                className="ui-field spec-editor"
                value={editorText(state, current)}
                readOnly={drafting}
                spellCheck={false}
                onChange={(e) => edit(task.id, current, e.target.value)}
              />
            </div>
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
