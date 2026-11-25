import { useState } from "react";
import { api } from "../../lib/api";
import { isHenchman } from "../../lib/lair";
import { useStore } from "../../store";
import { AgentAndModel } from "../AgentAndModel";
import { BusyLabel, ErrorMessage, Field, Switch, useFailure } from "../ui";

/** The efforts offered: the ones agents share. One its model lacks is left as the agent has it. */
const EFFORTS = [
  ["low", "Low"],
  ["medium", "Medium"],
  ["high", "High"],
] as const;

/**
 * Settings → Agents: whether there is a Henchman at all (HEN-16), what it
 * is woken with (HEN-15), and the user's own instructions for it (HEN-17).
 * Any agent that runs as a conversation, on the model and effort chosen
 * here, which it opens on each time, put back at launch too.
 */
export function HenchmanSettings() {
  const chosen = useStore((s) => s.settings?.henchman);
  const on = useStore((s) => s.settings?.henchman_on);
  const agents = useStore((s) => s.agents);
  const refreshSettings = useStore((s) => s.refreshSettings);
  const refreshPanes = useStore((s) => s.refreshPanes);
  const running = useStore((s) => s.panes.find((p) => p.running && isHenchman(p)));
  const failure = useFailure();
  const [switching, setSwitching] = useState(false);
  const [turning, setTurning] = useState(false);
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

  // Off stops the one running, which waits up to five seconds for it to save.
  async function turn(value: boolean) {
    if (turning) return;
    setTurning(true);
    failure.clear();
    try {
      await api.setHenchmanOn(value);
      await Promise.all([refreshSettings(), refreshPanes(), useStore.getState().refreshWaiting()]);
    } catch (e) {
      failure.fail(e);
    } finally {
      setTurning(false);
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
      <div className="switch-list">
        <Switch
          label="Keep a Henchman over your agents"
          detail="It reads every agent and chat, answers what you told it to, and pings you only when something needs you. Off: it stops and keeps its conversation, and the agents ping you themselves."
          checked={on ?? true}
          disabled={turning}
          onChange={(v) => void turn(v)}
        />
      </div>
      {/* What it is woken with: nothing to choose while it is off. */}
      {on !== false && (
        <>
          <AgentAndModel
            agentId={chosen.agent_id}
            model={chosen.model}
            conversational={conversational}
            agentHint="Who it is woken with."
            modelHint="Each time it opens, put back at launch too."
            // Another agent's efforts are its own too: back to it deciding.
            onChange={(id, model) =>
              void save(id, model, id === chosen.agent_id ? chosen.effort : "")
            }
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
            It reads Layer and passes your words on, and writes no code: a smaller model does it
            well.
          </div>
          {running && elsewhere && (
            <div className="row" style={{ marginTop: 10, gap: 12 }}>
              <span className="muted">
                The Henchman running now is on {nameOf(running.agent_id)}. Woken with{" "}
                {elsewhere.name}, it begins a new conversation.
              </span>
              <div className="spacer" />
              <button
                className="ui-button"
                disabled={switching}
                onClick={() => void switchNow(elsewhere.id)}
              >
                <BusyLabel
                  busy={switching}
                  idle={`Wake it with ${elsewhere.name}`}
                  working="Waking…"
                />
              </button>
            </div>
          )}
          <Instructions />
        </>
      )}
      <ErrorMessage>{failure.error}</ErrorMessage>
    </Field>
  );
}

/** The most the app keeps (HEN-17). */
const INSTRUCTIONS_MAX = 4000;

/**
 * The user's own instructions for the Henchman (HEN-17). Saved when the
 * field is left, or on ⌘Enter, with no Save button
 * (studio/design/behaviour.md), like the Slack bot's personality.
 */
function Instructions() {
  const saved = useStore((s) => s.settings?.henchman_instructions) ?? "";
  const refreshSettings = useStore((s) => s.refreshSettings);
  // Null until edited, so the field shows what is saved, not a stale copy.
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const failure = useFailure();

  async function save(value: string) {
    if (saving) return;
    if (value.trim() === saved) return setDraft(null);
    setSaving(true);
    failure.clear();
    try {
      await api.setHenchmanInstructions(value);
      await refreshSettings();
      setDraft(null);
    } catch (e) {
      failure.fail(e);
    } finally {
      setSaving(false);
    }
  }

  // Its own gap: the hint above it has none below.
  return (
    <div style={{ marginTop: 12 }}>
      <Field
        label="Custom instructions"
        hint="In your words, after Layer's own rules: what to raise, what to leave, how to write to you. The one running reads them when it is next woken."
      >
        <textarea
          // Field's label is not tied to it: named here for VoiceOver.
          aria-label="Custom instructions"
          className="ui-field"
          rows={4}
          maxLength={INSTRUCTIONS_MAX}
          value={draft ?? saved}
          readOnly={saving}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={(e) => void save(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && e.metaKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              e.currentTarget.blur();
            }
          }}
          placeholder="Raise only what blocks a release. Keep each update to a line or two."
        />
        <ErrorMessage>{failure.error}</ErrorMessage>
      </Field>
    </div>
  );
}
