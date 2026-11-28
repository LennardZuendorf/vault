import { useId, useRef, useState } from "react";
import { rejectInvalidTextTransfer, tagError } from "../lib/item-details-editor";
import { useStore } from "../store";
import { BusyLabel, ErrorMessage, Field, useFailure } from "./ui";

export function TagsSettings() {
  const tags = useStore((s) => s.tags);
  const addTag = useStore((s) => s.addTag);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const failure = useFailure();
  const id = useId();
  async function add(e: React.FormEvent) {
    e.preventDefault();
    if (running.current) return;
    const error = tagError(text);
    setInvalid(error);
    failure.clear();
    if (error) return;
    running.current = true;
    setBusy(true);
    try {
      await addTag(text);
      setText("");
    } catch (error) {
      failure.fail(error);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }
  return (
    <Field label="Tags" hint="Reusable text tags for chats and work in this profile.">
      <form className="browser-add" onSubmit={(e) => void add(e)}>
        <input
          className="ui-field"
          aria-label="New tag"
          value={text}
          disabled={busy}
          aria-invalid={!!invalid}
          aria-describedby={invalid ? id : undefined}
          onPaste={(e) =>
            rejectInvalidTextTransfer(e.clipboardData.getData("text/plain"), e, setInvalid)
          }
          onDrop={(e) =>
            rejectInvalidTextTransfer(e.dataTransfer.getData("text/plain"), e, setInvalid)
          }
          onChange={(e) => {
            setText(e.target.value);
            setInvalid(null);
          }}
        />
        <button className="ui-button" disabled={busy || !text.trim()}>
          <BusyLabel busy={busy} idle="Add tag" working="Adding…" />
        </button>
      </form>
      <div id={id}>
        <ErrorMessage>{invalid}</ErrorMessage>
      </div>
      <ErrorMessage>{failure.error}</ErrorMessage>
      {tags.length ? (
        <ul className="site-chips" aria-label="Tag catalog">
          {tags.map((tag) => (
            <li key={tag} className="ui-chip site-chip">
              {tag}
            </li>
          ))}
        </ul>
      ) : (
        <p className="hint">No tags yet. Add a tag to reuse it in chats and work.</p>
      )}
    </Field>
  );
}
