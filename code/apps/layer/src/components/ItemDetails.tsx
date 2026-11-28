import { useId, useRef, useState } from "react";
import {
  detailsDraft,
  detailsPatch,
  ITEM_COLORS,
  normalizeTags,
  rejectInvalidTextTransfer,
} from "../lib/item-details-editor";
import type { DetailsPatch, ItemDetails } from "../lib/types";
import { useStore } from "../store";
import { BusyLabel, ErrorMessage, Field, Modal, useFailure } from "./ui";
import "./item-details.css";

export interface ItemDetailsEditorProps {
  details?: ItemDetails;
  automaticName: string;
  onSave: (patch: DetailsPatch) => Promise<unknown>;
  onClose: () => void;
}

/** Mount one editor per item. Its opening snapshot must not follow list refreshes. */
export function ItemDetailsEditor({
  details,
  automaticName,
  onSave,
  onClose,
}: ItemDetailsEditorProps) {
  const [initial] = useState(() => details && structuredClone(details));
  const [draft, setDraft] = useState(() => detailsDraft(details));
  const catalog = useStore((s) => s.tags);
  const [adding, setAdding] = useState("");
  const [tagIssue, setTagIssue] = useState<string | null>(null);
  const [nameIssue, setNameIssue] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const failure = useFailure();
  const id = useId();
  const close = () => {
    if (!running.current) onClose();
  };

  function withTag(): string[] | null {
    if (!adding) return draft.tags;
    try {
      return normalizeTags([...draft.tags, adding], catalog);
    } catch (error) {
      setTagIssue((error as Error).message);
      return null;
    }
  }
  function add() {
    const tags = withTag();
    if (tags) {
      setDraft({ ...draft, tags });
      setAdding("");
      setTagIssue(null);
    }
  }
  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (running.current) return;
    failure.clear();
    setNameIssue(null);
    const tags = withTag();
    if (!tags) return;
    let patch: DetailsPatch;
    try {
      // Field errors stay beside the relevant control.
      detailsPatch(initial, { ...draft, tags: initial?.tags ?? [] }, catalog);
    } catch (error) {
      setNameIssue((error as Error).message);
      return;
    }
    try {
      patch = detailsPatch(initial, { ...draft, tags }, catalog);
    } catch (error) {
      setTagIssue((error as Error).message);
      return;
    }
    running.current = true;
    setBusy(true);
    try {
      await onSave(patch);
      onClose();
    } catch (error) {
      failure.fail(error);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }
  return (
    <Modal title="Edit details" onClose={close}>
      <form
        className="item-details-editor"
        onSubmit={(e) => void save(e)}
        aria-busy={busy}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            if (!running.current) e.currentTarget.requestSubmit();
          }
        }}
      >
        <fieldset disabled={busy} className="item-details-fields">
          <Field label="Display name" hint="Leave blank to use the automatic name.">
            <input
              className="ui-field"
              aria-label="Display name"
              placeholder={automaticName}
              value={draft.display_name}
              aria-invalid={!!nameIssue}
              aria-describedby={nameIssue ? `${id}-name` : undefined}
              onPaste={(e) =>
                rejectInvalidTextTransfer(e.clipboardData.getData("text/plain"), e, setNameIssue)
              }
              onDrop={(e) =>
                rejectInvalidTextTransfer(e.dataTransfer.getData("text/plain"), e, setNameIssue)
              }
              onChange={(e) => {
                setDraft({ ...draft, display_name: e.target.value });
                setNameIssue(null);
              }}
            />
            <div id={`${id}-name`}>
              <ErrorMessage>{nameIssue}</ErrorMessage>
            </div>
          </Field>
          <Field label="Tags">
            <div className="item-details-tags">
              {draft.tags.map((tag) => (
                <button
                  type="button"
                  key={tag}
                  className="ui-chip item-details-tag"
                  aria-label={`Remove tag ${tag}`}
                  onClick={() => setDraft({ ...draft, tags: draft.tags.filter((t) => t !== tag) })}
                >
                  {tag} ×
                </button>
              ))}
            </div>
            <div className="browser-add">
              <input
                className="ui-field"
                aria-label="Add a tag"
                list={`${id}-catalog`}
                value={adding}
                aria-invalid={!!tagIssue}
                aria-describedby={tagIssue ? `${id}-tags` : undefined}
                onPaste={(e) =>
                  rejectInvalidTextTransfer(e.clipboardData.getData("text/plain"), e, setTagIssue)
                }
                onDrop={(e) =>
                  rejectInvalidTextTransfer(e.dataTransfer.getData("text/plain"), e, setTagIssue)
                }
                onChange={(e) => {
                  setAdding(e.target.value);
                  setTagIssue(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    add();
                  }
                }}
              />
              <datalist id={`${id}-catalog`}>
                {catalog.map((tag) => (
                  <option key={tag} value={tag}>
                    {tag}
                  </option>
                ))}
              </datalist>
              <button type="button" className="ui-button" disabled={!adding.trim()} onClick={add}>
                Add tag
              </button>
            </div>
            <div id={`${id}-tags`}>
              <ErrorMessage>{tagIssue}</ErrorMessage>
            </div>
          </Field>
          <Field label="Color">
            <select
              className="ui-field"
              aria-label="Color"
              value={draft.color ?? ""}
              onChange={(e) => setDraft({ ...draft, color: e.target.value || null })}
            >
              {ITEM_COLORS.map(({ value, label }) => (
                <option key={value ?? ""} value={value ?? ""}>
                  {label}
                </option>
              ))}
            </select>
          </Field>
        </fieldset>
        <ErrorMessage>{failure.error}</ErrorMessage>
        <div className="item-details-actions">
          <button type="button" className="ui-button" disabled={busy} onClick={close}>
            Cancel
          </button>
          <button className="ui-button primary" disabled={busy}>
            <BusyLabel busy={busy} idle="Save" working="Saving…" />
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** No wrapper for empty details: unchanged rows keep their original appearance. */
export function ItemDetailsIndicators({ details }: { details?: ItemDetails }) {
  if (!details?.tags.length && !details?.color) return null;
  const color = ITEM_COLORS.find((c) => c.value === details?.color);
  return (
    <span className="item-details-indicators">
      {details?.color && color && (
        <span
          className="item-details-color"
          role="img"
          aria-label={`Color: ${color.label}`}
          title={`Color: ${color.label}`}
          style={{ backgroundColor: `var(--${details.color})` }}
        />
      )}
      {details?.tags.map((tag) => (
        <span key={tag} className="ui-chip item-details-tag">
          {tag}
        </span>
      ))}
    </span>
  );
}
