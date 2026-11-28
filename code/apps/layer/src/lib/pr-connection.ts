import type { CheckoutPr, Settings } from "./types";

export function prConnectionScope(settings: Settings | null): string | undefined {
  if (!settings) return undefined;
  return JSON.stringify([
    settings.github_connected,
    settings.github?.api_url,
    settings.github?.web_url,
  ]);
}

/** Retain the local association, not URLs or evidence from the previous forge. */
export function invalidatePrRows(prs: Record<string, CheckoutPr[]>): Record<string, CheckoutPr[]> {
  return Object.fromEntries(
    Object.entries(prs).map(([task, rows]) => [
      task,
      rows.map((row) => ({
        ...row,
        pin: row.pin && { ...row.pin, url: "" },
        pin_read: false,
        pr: null,
        past: [],
        checks: [],
        reviews: [],
        verdict: "none",
        to_reply: 0,
        error: "The GitHub connection changed. Refresh to read this repository again.",
        branch_covered: false,
        detached_unlanded: false,
        covered_merge: false,
        action_target: null,
      })),
    ]),
  );
}
