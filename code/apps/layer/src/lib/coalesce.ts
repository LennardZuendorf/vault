export interface Slot {
  get: () => Promise<void> | null;
  set: (p: Promise<void> | null) => void;
  /** The one refresh queued behind the call in flight, shared by everyone who asked meanwhile. */
  next?: Promise<void> | null;
}

/**
 * Run `work` once, sharing it with polls that overlap and queueing behind it
 * for anything that is not a poll.
 *
 * Everything that queues while a call is out shares one refresh after it:
 * each hook post announces itself, and five of them during one call became
 * five calls in a row, each waiting for the last.
 */
export async function coalesce(
  slot: Slot,
  poll: boolean,
  work: () => Promise<void>,
): Promise<void> {
  const current = slot.get();
  if (current) {
    if (poll) return current;
    // Wait the in-flight one out, then ask again: what it returns was asked
    // for before whatever just changed.
    if (!slot.next) {
      slot.next = current
        .catch(() => {})
        .then(() => {
          slot.next = null;
          return coalesce(slot, false, work);
        });
    }
    return slot.next;
  }
  const run = work();
  slot.set(run);
  try {
    await run;
  } finally {
    if (slot.get() === run) slot.set(null);
  }
}
