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
