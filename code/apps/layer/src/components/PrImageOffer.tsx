/**
 * Screenshots an agent offered for a pull request with `pr_images`, waiting
 * for the user to add them (PR-13). The agent's own yes was refused by
 * Claude Code's auto mode as a possible leak; here the user sees each image
 * before anything is stored in the repository.
 */
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { PrImageOffer as Offer } from "../lib/types";

export function PrImageOffer({
  taskId,
  fail,
  onAdded,
}: {
  taskId: string;
  fail: (e: unknown) => void;
  onAdded: () => void;
}) {
  const [offer, setOffer] = useState<Offer | null>(null);
  const [adding, setAdding] = useState(false);

  const read = useCallback(() => {
    api
      .prImageOffer(taskId)
      .then(setOffer)
      .catch(() => {});
  }, [taskId]);
  useEffect(() => {
    read();
    const p = listen<string>("pr:images-offered", (e) => {
      if (e.payload === taskId) read();
    });
    return () => {
      void p.then((un) => un());
    };
  }, [taskId, read]);

  if (!offer || offer.images.length === 0) return null;

  const dismiss = () => {
    setOffer(null);
    void api.dropPrImageOffer(taskId).catch(fail);
  };
  const add = async () => {
    setAdding(true);
    try {
      const rows = await api.showPrImages(taskId, offer.images);
      const failed = rows.filter((r) => !r.ok);
      if (failed.length > 0) {
        fail(failed.map((r) => `${r.repo}: ${r.detail}`).join("\n"));
        return;
      }
      await api.dropPrImageOffer(taskId);
      setOffer(null);
      onAdded();
    } catch (e) {
      fail(e);
    } finally {
      setAdding(false);
    }
  };

  return (
    <div className="card">
      <div className="row">
        <h3 style={{ margin: 0 }}>Screenshots for {offer.repo}'s pull request</h3>
        <div className="spacer" />
        <button className="ui-button" onClick={dismiss} disabled={adding}>
          Dismiss
        </button>
        <button
          className="ui-button ui-button-primary"
          onClick={() => void add()}
          disabled={adding}
        >
          {adding ? "Adding…" : "Add to pull request"}
        </button>
      </div>
      <div className="pr-shots">
        <div className="pr-shots-list">
          {offer.images.map((i) => (
            <figure key={i.path} className="pr-shot">
              {i.preview && <img src={i.preview} alt={i.caption} />}
              <figcaption>{i.caption}</figcaption>
              <button
                className="pr-shot-remove"
                title="Leave out"
                aria-label={`Leave out ${i.caption}`}
                onClick={() =>
                  setOffer({ ...offer, images: offer.images.filter((x) => x.path !== i.path) })
                }
              >
                ×
              </button>
            </figure>
          ))}
        </div>
        <div className="muted">
          An agent asked to show these in the pull request. Added, they are stored in the repository
          for everyone who can read it.
        </div>
      </div>
    </div>
  );
}
