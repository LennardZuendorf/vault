import type { AcpCommand } from "./types";

/**
 * Layer's own commands (ACP-22). An agent that has one by the same name
 * keeps it: the backend sends that one to the agent.
 */
const LAYERS: AcpCommand[] = [
  { name: "clear", description: "Start a new conversation, same model and mode", hint: null },
  { name: "new", description: "Start a new conversation, same model and mode", hint: null },
];

/**
 * The commands to offer under a draft that is a bare `/name` being typed:
 * the agent's, then Layer's own it does not have, at most eight.
 */
export function suggestedCommands(theirs: AcpCommand[], draft: string): AcpCommand[] {
  if (!draft.startsWith("/") || draft.includes(" ")) return [];
  const all = [...theirs, ...LAYERS.filter((l) => !theirs.some((c) => c.name === l.name))];
  return all.filter((c) => c.name.startsWith(draft.slice(1))).slice(0, 8);
}
