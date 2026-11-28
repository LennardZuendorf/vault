import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { api, errMessage } from "../lib/api";
import type { CheckoutPr } from "../lib/types";
import { PR_PIN_REVISION_CHANGED } from "../lib/types-github";
import { useStore } from "../store";
import { BusyLabel, ErrorMessage, useFailure } from "./ui";

export function PrPin({
  taskId,
  row,
  onStale,
  unavailable = false,
  showLink = true,
}: {
  taskId: string;
  row: CheckoutPr;
  onStale: (error?: unknown) => void;
  unavailable?: boolean;
  showLink?: boolean;
}) {
  const { error, clear, fail } = useFailure();
  const [busy, setBusy] = useState(false);
  const pin = row.pin;
  if (!pin) return null;
  const name = `${row.repo} #${pin.number}`;
  const url = showLink ? pin.url : "";

  async function unpin() {
    clear();
    onStale();
    setBusy(true);
    try {
      // The saved pin change emits the same refresh event as the agent's tool.
      await api.unpinTaskPr(taskId, row.checkout_id, row.pin_revision);
    } catch (e) {
      if (errMessage(e) === PR_PIN_REVISION_CHANGED) {
        onStale(e);
        await useStore
          .getState()
          .refreshTaskPrs(taskId)
          .catch(() => {});
      } else {
        fail(e);
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <span className="ui-chip">Pinned</span>
        {unavailable &&
          (url ? (
            <button
              className="ui-link"
              onClick={() =>
                void openUrl(url).catch(() => fail("Couldn't open the browser. Try again."))
              }
            >
              {name}
            </button>
          ) : (
            <span>{name}</span>
          ))}
        <button
          className="ui-link"
          disabled={busy}
          aria-label={`Unpin ${name}`}
          title="Remove this pin from Layer without changing GitHub"
          onClick={() => void unpin()}
        >
          <BusyLabel busy={busy} idle="Unpin" working="Unpinning…" />
        </button>
      </div>
      {unavailable && (
        <div className="muted" style={{ overflowWrap: "anywhere" }}>
          {showLink ? row.error : "GitHub is not connected. Connect it to read pull requests."}
        </div>
      )}
      <ErrorMessage>{error}</ErrorMessage>
    </div>
  );
}
