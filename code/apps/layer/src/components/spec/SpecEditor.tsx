import { editorText, useSpecs } from "../../lib/specs";
import type { SpecFileView, TaskView } from "../../lib/types";
import { ErrorMessage, useFailure } from "../ui";

/**
 * One file of the spec in an editor, saved as a plain file write, the way an
 * agent writes it (SPEC-28). When the file changes on disk under an editor
 * with unsaved text, the text stays and a line says so: Load theirs takes the
 * file's, Save mine writes over it. There is no merge.
 */
export function SpecEditor({
  task,
  file,
  path,
  readOnly,
}: {
  task: TaskView;
  /** The file as the backend has it, none for one only a draft is making. */
  file: SpecFileView | undefined;
  path: string;
  readOnly: boolean;
}) {
  const state = useSpecs((s) => s.byTask[task.id]);
  const edit = useSpecs((s) => s.edit);
  const save = useSpecs((s) => s.save);
  const discard = useSpecs((s) => s.discard);
  const failure = useFailure();
  const pending = state?.edits[path];
  const conflict = pending?.conflict ?? false;

  async function saveNow() {
    if (!pending || readOnly) return;
    failure.clear();
    try {
      await save(task.id, path);
    } catch (e) {
      failure.fail(e);
    }
  }

  return (
    <div className="spec-editor-pane">
      <div className="spec-bar">
        <span className="spec-path">{path}</span>
        <span className="spacer" />
        {pending && !readOnly && (
          <>
            {conflict && (
              <button className="ui-button" onClick={() => discard(task.id, path)}>
                Load theirs
              </button>
            )}
            {!conflict && (
              <button className="ui-button" onClick={() => discard(task.id, path)}>
                Discard changes
              </button>
            )}
            <button className="ui-button ui-button-primary" onClick={() => void saveNow()}>
              {conflict ? "Save mine" : "Save"}
            </button>
          </>
        )}
      </div>
      {conflict && (
        <p className="spec-conflict" role="status">
          {file ? `${path} changed on disk while you were editing it.` : `${path} was removed.`}{" "}
          Your text is kept.
        </p>
      )}
      <ErrorMessage>{failure.error}</ErrorMessage>
      <textarea
        className="ui-field spec-editor"
        aria-label={path}
        value={editorText(state, path)}
        readOnly={readOnly}
        spellCheck={false}
        onChange={(e) => edit(task.id, path, e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && (e.key === "Enter" || e.key === "s")) {
            e.preventDefault();
            void saveNow();
          }
        }}
      />
    </div>
  );
}
