import { useState } from "react";
import { api } from "../../lib/api";
import { isHenchman } from "../../lib/lair";
import { useStore } from "../../store";
import { AgentAndModel } from "../AgentAndModel";
import { BusyLabel, ErrorMessage, Field, useFailure } from "../ui";

/** The efforts offered: the ones agents share. One its model lacks is left as the agent has it. */
const EFFORTS = [
  ["low", "Low"],
  ["medium", "Medium"],
  ["high", "High"],
] as const;

/**
 * Settings → Agents: what the Henchman is woken with (HEN-15). Any agent
 * that runs as a conversation, on the model and effort chosen here, which
 * it opens on each time, put back at launch too.
 */
export function HenchmanSettings() {
  const chosen = useStore((s) => s.settings?.henchman);
  const agents = useStore((s) => s.agents);
  const refreshSettings = useStore((s) => s.refreshSettings);
  const refreshPanes = useStore((s) => s.refreshPanes);
  const running = useStore((s) => s.panes.find((p) => p.running && isHenchman(p)));
  const failure = useFailure();
  const [switching, setSwitching] = useState(false);
  if (!chosen) return null;

  const conversational = agents.filter((a) => a.installed && a.acp);
  const nameOf = (id: string | null) => agents.find((a) => a.id === id)?.name ?? id ?? "";
  // The one running keeps its agent: a conversation cannot move to another.
  const elsewhere =
    running && running.agent_id !== chosen.agent_id
      ? conversational.find((a) => a.id === chosen.agent_id)
      : undefined;

  async function save(agentId: string, model: string, effort: string) {
    failure.clear();
    try {
      await api.setHenchmanAgent(agentId, model, effort);
      await refreshSettings();
    } catch (e) {
      failure.fail(e);
    }
  }

  async function switchNow(agentId: string) {
    if (!running || switching) return;
    setSwitching(true);
    failure.clear();
    try {
      await api.closePane(running.id);
      await api.spawnHenchman(agentId);
      await refreshPanes();
    } catch (e) {
      failure.fail(e);
    } finally {
      setSwitching(false);
    }
  }

  return (
    <Field label="Henchman">
      <AgentAndModel
        agentId={chosen.agent_id}
        model={chosen.model}
        conversational={conversational}
        agentHint="Who it is woken with."
        modelHint="Each time it opens, put back at launch too."
        // Another agent's efforts are its own too: back to it deciding.
        onChange={(id, model) => void save(id, model, id === chosen.agent_id ? chosen.effort : "")}
      >
        <Field label="Effort" hint="Where its model has one.">
          <select
            className="ui-field"
            value={chosen.effort}
            onChange={(e) => void save(chosen.agent_id, chosen.model, e.target.value)}
          >
            <option value="">{nameOf(chosen.agent_id)} decides</option>
            {EFFORTS.map(([value, name]) => (
              <option key={value} value={value}>
                {name}
              </option>
            ))}
          </select>
        </Field>
      </AgentAndModel>
      {/* Above the switch, not under it as Field's own hint would be. */}
      <div className="hint">
        It reads Layer and passes your words on, and writes no code: a smaller model does it well.
      </div>
      {running && elsewhere && (
        <div className="row" style={{ marginTop: 10, gap: 12 }}>
          <span className="muted">
            The Henchman running now is on {nameOf(running.agent_id)}. Woken with {elsewhere.name},
            it begins a new conversation.
          </span>
          <div className="spacer" />
          <button
            className="ui-button"
            disabled={switching}
            onClick={() => void switchNow(elsewhere.id)}
          >
            <BusyLabel busy={switching} idle={`Wake it with ${elsewhere.name}`} working="Waking…" />
          </button>
        </div>
      )}
      <ErrorMessage>{failure.error}</ErrorMessage>
    </Field>
  );
}
