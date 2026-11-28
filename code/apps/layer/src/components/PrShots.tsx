/**
 * Screenshots on their way to a pull request (PR-4, PR-13): a row of
 * previews, each with a way to leave it out, and a click opens one full
 * size. At 96px high a mobile and a desktop screenshot looked alike, and
 * the user had to add them without reading what they showed.
 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import type { DraftImage } from "../lib/types";

export function PrShots({
  images,
  label = (i) => i.caption,
  onRemove,
  note,
}: {
  images: DraftImage[];
  label?: (i: DraftImage) => string;
  onRemove: (path: string) => void;
  note: string;
}) {
  const [open, setOpen] = useState<number | null>(null);
  const shown = images.filter((i) => i.preview);
  return (
    <div className="pr-shots">
      <div className="pr-shots-list">
        {images.map((i) => (
          <figure key={i.path} className="pr-shot">
            {i.preview && (
              <button
                className="pr-shot-open"
                title="See it full size"
                aria-label={`See ${i.caption} full size`}
                onClick={() => setOpen(shown.indexOf(i))}
              >
                <img src={i.preview} alt={i.caption} />
              </button>
            )}
            <figcaption>{label(i)}</figcaption>
            <button
              className="pr-shot-remove"
              title="Leave out"
              aria-label={`Leave out ${i.caption}`}
              onClick={() => onRemove(i.path)}
            >
              ×
            </button>
          </figure>
        ))}
      </div>
      <div className="muted">{note}</div>
      {open !== null && shown[open] && (
        <ShotViewer images={shown} at={open} label={label} onMove={setOpen} />
      )}
    </div>
  );
}

/** One screenshot over the window: ← and → step through, Esc or a click closes. */
function ShotViewer({
  images,
  at,
  label,
  onMove,
}: {
  images: DraftImage[];
  at: number;
  label: (i: DraftImage) => string;
  onMove: (at: number | null) => void;
}) {
  useEffect(() => {
    // Capture, and stopped there: the PR dialog under it closes on Esc too.
    const onKey = (e: KeyboardEvent) => {
      const step = { ArrowLeft: -1, ArrowRight: 1 }[e.key];
      if (e.key !== "Escape" && step === undefined) return;
      e.stopPropagation();
      e.preventDefault();
      onMove(step === undefined ? null : (at + step + images.length) % images.length);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [at, images.length, onMove]);

  const image = images[at];
  // Over everything, outside the dialog's box and the shell's zoom.
  return createPortal(
    // oxlint-disable-next-line jsx-a11y/click-events-have-key-events -- the backdrop: Esc closes it from the keyboard
    <div
      className="ui-backdrop overlay shot-viewer"
      role="dialog"
      aria-modal="true"
      aria-label={image.caption}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={() => onMove(null)}
    >
      <img src={image.preview} alt={image.caption} />
      <div className="shot-viewer-caption">
        {label(image)}
        {images.length > 1 && (
          <span className="muted">
            {" "}
            · {at + 1} of {images.length} · ← →
          </span>
        )}
      </div>
    </div>,
    document.body,
  );
}
