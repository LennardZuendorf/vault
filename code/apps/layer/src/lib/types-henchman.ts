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
