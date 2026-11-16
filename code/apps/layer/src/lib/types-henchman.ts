/** One process a pane started that still runs (HEN-6). `Process` in `commands/processes.rs`. */
export interface Process {
  pid: number;
  pane_id: string;
  task_id: string;
  /** Who started it: the task's ticket key or name, or the chat's name. */
  owner: string;
  /** The program's name and its arguments. */
  command: string;
  /** How long it has run, in seconds. */
  secs: number;
  /** The TCP ports it, or anything under it, listens on. */
  ports: number[];
}

/** One thing the user told the Henchman to always do (HEN-7). `StandingOrder` in `config/henchman.rs`. */
export interface StandingOrder {
  id: string;
  text: string;
}

/** A task or chat the Henchman leaves alone (HEN-8). */
export interface LeftAlone {
  /** The task's id, or the chat's room. */
  key: string;
  /** Its ticket key or name; the chat's name. */
  name: string;
}

/** `Orders` in `commands/henchman_orders.rs`. */
export interface HenchmanOrders {
  orders: StandingOrder[];
  left_alone: LeftAlone[];
}

/** Something the Henchman needs the user for (HEN-13). `Need` in `commands/henchman_need.rs`. */
export interface HenchmanNeed {
  id: string;
  at: string;
  /** The Henchman's pane. */
  paneId: string;
  /** The task it is about, which Open opens. */
  taskId: string | null;
  /** Its ticket key, or its name. */
  key: string | null;
  text: string;
  /** The user has looked at the Henchman's conversation since: off the dock, still listed. */
  seen: boolean;
}

/** One thing the Henchman did for the user (HEN-14). `Did` in `commands/henchman_lair.rs`. */
export interface HenchmanDid {
  at: string;
  taskId: string | null;
  key: string | null;
  what: string;
}

/** What the lair keeps across a restart (HEN-14). `Lair` in `commands/henchman_lair.rs`. */
export interface HenchmanLair {
  needs: HenchmanNeed[];
  /** Newest first. */
  did: HenchmanDid[];
}

/**
 * The agent the Henchman is woken with, and the model and effort it opens
 * on; empty is the agent's own (HEN-15). `HenchmanAgent` in `config/henchman.rs`.
 */
export interface HenchmanAgent {
  agent_id: string;
  model: string;
  effort: string;
}
