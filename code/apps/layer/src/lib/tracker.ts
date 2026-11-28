import type { TrackerKind } from "@layer/types";
import { useStore } from "../store";
import type { Settings } from "./types";

/** A tracker by its own name, for "Open in …" and the like (TKT-17). */
export function trackerName(kind: TrackerKind | null | undefined): string {
  return kind === "github" ? "GitHub" : "Jira";
}

/** The name of the tracker tickets come from now. */
export function useTrackerName(): string {
  return trackerName(useStore((s) => s.settings?.tracker_chosen));
}

/**
 * Where a ticket is seen in its tracker, from its key alone, as the backend's
 * `tickets::url_of` makes it: an epic known only through its children has
 * no ticket of its own to read the link from.
 */
export function ticketLink(settings: Settings | null, key: string): string | null {
  if (settings?.tracker_chosen === "github") {
    const m = /^([\w.-]+\/[\w.-]+)#(\d+)$/.exec(key);
    const web = settings.github?.web_url.replace(/\/+$/, "");
    return m && web ? `${web}/${m[1]}/issues/${m[2]}` : null;
  }
  const base = settings?.jira?.base_url.replace(/\/+$/, "");
  return base ? `${base}/browse/${key}` : null;
}

/** UI identity only; writes use the backend's credential generation as well. */
export function ticketConnectionScope(settings: Settings | null): string | null {
  switch (settings?.tracker) {
    case "jira":
      return JSON.stringify(["jira", settings.jira?.base_url, settings.jira?.email]);
    case "github":
      return JSON.stringify(["github", settings.github?.api_url, settings.github?.web_url]);
    default:
      return null;
  }
}

/** Normalize the authority without conflating paths, queries, or non-default ports. */
export function ticketUrlIdentity(raw: string | null | undefined): string | null {
  if (!raw) return null;
  try {
    const url = new URL(raw);
    if ((url.protocol === "https:" || url.protocol === "http:") && !url.username && !url.password)
      return `${url.origin}${url.pathname}${url.search}`;
  } catch {
    // An invalid or non-web URL cannot match the active tracker.
  }
  return null;
}

export function sameTicketUrl(a: string | null | undefined, b: string | null | undefined): boolean {
  const identity = ticketUrlIdentity(a);
  return identity !== null && identity === ticketUrlIdentity(b);
}

/** A key may exist on two sites. Keep the original link, but only read status from its site. */
export function readsTicket(settings: Settings | null, key: string, url: string): boolean {
  return (
    settings?.tracker === (key.includes("#") ? "github" : "jira") &&
    sameTicketUrl(ticketLink(settings, key), url)
  );
}

/**
 * The tracker a key belongs to, whatever the workspace uses now: a task
 * keeps its ticket when the choice changes (TKT-16), and its link should
 * still say where it goes.
 */
export function trackerOfKey(key: string): string {
  return trackerName(key.includes("#") ? "github" : "jira");
}

/** Where a ticket is filed: a Jira project's key, or a GitHub repository. */
export function projectOf(key: string): string {
  const hash = key.indexOf("#");
  return hash > 0 ? key.slice(0, hash) : (key.split("-")[0] ?? "");
}

/**
 * The start of a ticket's branch, as `derive_branch` makes it: a Jira key
 * as it is; for a GitHub issue its number and its title's slug, since
 * `owner/repo#12` is no branch name (TKT-21).
 */
export function branchStem(key: string, summary: string): string {
  const hash = key.lastIndexOf("#");
  if (hash < 0) return key;
  const slug = summary
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
  return [key.slice(hash + 1), slug].filter(Boolean).join("-");
}
