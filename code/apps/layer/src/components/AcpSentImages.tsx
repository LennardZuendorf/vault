import { useState } from "react";
import type { AcpImage } from "../lib/types";
import { Modal } from "./ui";

/* oxlint-disable react/no-array-index-key -- pasted images have no ids, only their order */

/**
 * What the user sent with their words (ACP-16), small; a click shows one
 * whole. It was only named "[image]", so a screenshot sent an hour ago could
 * not be told from another.
 */
export function SentImages({ images }: { images: AcpImage[] }) {
  const [open, setOpen] = useState<number | null>(null);
  const url = (img: AcpImage) => `data:${img.mimeType};base64,${img.data}`;
  return (
    <div className="acp-images">
      {images.map((img, i) => (
        <button key={i} className="acp-sent-image" title="Show it whole" onClick={() => setOpen(i)}>
          <img src={url(img)} alt={`Sent ${i + 1}`} />
        </button>
      ))}
      {open !== null && images[open] && (
        <Modal
          title={images.length > 1 ? `Image ${open + 1} of ${images.length}` : "Image"}
          className="acp-image-view"
          onClose={() => setOpen(null)}
        >
          <img src={url(images[open])} alt={`Sent ${open + 1}`} />
        </Modal>
      )}
    </div>
  );
}
