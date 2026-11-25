import { useEffect } from "react";
import { failureText } from "../lib/api";

/** How long a failed download waits before the panel tries again (BRW-24). */
const RETRY_AFTER = 60_000;

const mb = (bytes: number) => Math.round(bytes / 2 ** 20);

/**
 * The app's own Chrome coming down, the first time the browser is needed
 * (BRW-24): in place of "Starting the browser", which it comes before.
 */
export function BrowserDownloading({ done, total }: { done: number; total: number }) {
  const share = total > 0 ? Math.min(100, (done / total) * 100) : null;
  return (
    <div className="browser-starting browser-download">
      <span>
        Downloading the browser
        {done > 0 && (total > 0 ? `: ${mb(done)} of ${mb(total)} MB` : `: ${mb(done)} MB`)}
      </span>
      <div
        className="browser-download-bar"
        role="progressbar"
        aria-label="Downloading the browser"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={share === null ? undefined : Math.round(share)}
      >
        <div style={{ width: `${share ?? 0}%` }} />
      </div>
      <p className="muted">
        Layer runs its own Chrome, apart from yours. It's downloaded once, and agents wait for it.
      </p>
    </div>
  );
}

/**
 * The download failed. A connection that comes back needs nothing from the
 * user (studio/design/behaviour.md), so there is no button: the panel asks
 * again in a minute.
 */
export function BrowserDownloadFailed({
  reason,
  onRetry,
}: {
  reason: string;
  onRetry: () => void;
}) {
  useEffect(() => {
    const t = window.setTimeout(onRetry, RETRY_AFTER);
    return () => window.clearTimeout(t);
  }, [reason]); // eslint-disable-line react-hooks/exhaustive-deps -- once per failure, not per render
  return (
    <div className="empty">
      <h2>Couldn't download the browser</h2>
      <p>{failureText(reason)}</p>
      <p className="muted">Layer tries again in a minute.</p>
    </div>
  );
}
