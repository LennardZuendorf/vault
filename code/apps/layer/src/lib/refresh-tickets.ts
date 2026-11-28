import type { StoreApi } from "zustand";
import type { State } from "../store";
import { api } from "./api";
import { keep, reuse } from "./same";
import { readsTicket, sameTicketUrl, ticketConnectionScope, ticketUrlIdentity } from "./tracker";
import type { Settings, Ticket } from "./types";

type LinkCache = State["linkedTickets"];

function ticketsByUrl(tickets: Ticket[]): Map<string, Ticket> {
  const byUrl = new Map<string, Ticket>();
  for (const ticket of tickets) {
    const id = ticketUrlIdentity(ticket.url);
    if (id) byUrl.set(id, ticket);
  }
  return byUrl;
}

function atTicketUrl(tickets: Map<string, Ticket>, url: string): Ticket | undefined {
  const id = ticketUrlIdentity(url);
  return id ? tickets.get(id) : undefined;
}

function withStatuses(
  cache: LinkCache,
  before: LinkCache,
  settings: Settings | null,
  tickets: Map<string, Ticket>,
): LinkCache {
  const updated = { ...cache };
  for (const link of Object.values(cache)) {
    const prior = before[link.task_id];
    // A link event applied mid-request may have fresher status. A newer
    // context warning on its own does not make this read stale.
    if (
      prior?.key !== link.key ||
      !sameTicketUrl(prior.url, link.url) ||
      prior.status !== link.status ||
      prior.status_category !== link.status_category
    )
      continue;
    if (!readsTicket(settings, link.key, link.url)) continue;
    const ticket = atTicketUrl(tickets, link.url);
    if (ticket?.key === link.key)
      updated[link.task_id] = {
        ...link,
        status: ticket.status,
        status_category: ticket.status_category,
      };
  }
  return keep(cache, updated);
}

async function readOutsideMine(keys: string[]): Promise<Ticket[]> {
  const reads: Promise<Ticket[]>[] = [];
  for (let i = 0; i < keys.length; i += 100)
    reads.push(api.ticketsByKey(keys.slice(i, i + 100)).catch(() => []));
  return (await Promise.all(reads)).flat();
}

/** Publish Mine on its own schedule; the Work header may need tickets that Mine excludes. */
export function issueRefresher(
  set: StoreApi<State>["setState"],
  get: StoreApi<State>["getState"],
): State["refreshIssues"] {
  let issueRead = 0;
  return async (opts) => {
    const settings = get().settings;
    const scope = ticketConnectionScope(settings);
    if (!scope) return;
    const before = get().linkedTickets;
    const read = ++issueRead;
    const current = () => read === issueRead && scope === ticketConnectionScope(get().settings);
    // A timer tick should not flash the spinner over a list already on screen.
    if (!opts?.quiet || !get().issuesLoaded) set({ issuesLoading: true });
    try {
      const [page, issueTypes, priorities] = await Promise.all([
        api.ticketsMine(),
        // Types rarely change and are cached in the backend; a failure here
        // must not stop the issues themselves from showing.
        api.ticketTypes().catch(() => get().issueTypes),
        // Asked once per connection, as rarely changed. Without them a sort
        // by priority keeps the order it had.
        get().priorities.length > 0 ? get().priorities : api.ticketPriorities().catch(() => []),
      ]);
      if (!current()) return;
      const mine = ticketsByUrl(page.issues);
      set((s) => ({
        issues: reuse(s.issues, page.issues, (i) => i.key),
        linkedTickets: withStatuses(s.linkedTickets, before, settings, mine),
        issuesTruncated: page.more,
        issueTypes: keep(s.issueTypes, issueTypes),
        priorities: keep(s.priorities, priorities),
        issuesLoaded: true,
        issuesLoading: false,
      }));

      // Mine drops Done and reassigned tickets. Read those separately, in
      // batches no larger than the tracker command's cap.
      const keys = [
        ...new Set(
          Object.values(before).flatMap((link) =>
            readsTicket(settings, link.key, link.url) &&
            atTicketUrl(mine, link.url)?.key !== link.key
              ? [link.key]
              : [],
          ),
        ),
      ];
      if (keys.length) {
        // Start Work and other callers wait for Mine, not these Work-only badges.
        void readOutsideMine(keys)
          .then((tickets) => {
            if (!current()) return;
            const outside = ticketsByUrl(tickets);
            set((s) => ({
              linkedTickets: withStatuses(s.linkedTickets, before, settings, outside),
            }));
          })
          .catch(() => {}); // A background failure leaves the last known status.
      }
    } catch (e) {
      if (!opts?.quiet && current()) get().fail(e);
    } finally {
      if (read === issueRead && get().issuesLoading) set({ issuesLoading: false });
    }
  };
}
