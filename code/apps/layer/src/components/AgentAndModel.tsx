import type { ReactNode } from "react";
import type { AgentStatus } from "../lib/types";
import { useStore } from "../store";
import { Field } from "./ui";

/**
 * An agent that runs as a conversation, and the model it runs on: the
 * Slack bot's (BOT-17) and the Henchman's (HEN-15). An agent names its
 * models only to a conversation, so the list is what it offered when one
 * last opened. Choosing another agent goes back to it deciding.
 */
export function AgentAndModel({
  agentId,
  model,
  conversational,
  agentHint,
  modelHint,
  onChange,
  children,
}: {
  agentId: string;
  model: string;
  /** The agents on this Mac that run as a conversation. */
  conversational: AgentStatus[];
  agentHint: string;
  /** Under the model once the agent has named its models. */
  modelHint: string;
  onChange: (agentId: string, model: string) => void;
  /** More fields on the same row. */
  children?: ReactNode;
}) {
  const offered = useStore((s) => s.settings?.agent_models[agentId]) ?? [];
  const agentName = conversational.find((a) => a.id === agentId)?.name ?? agentId;
  // A model saved that the agent no longer lists still shows as chosen.
  const models =
    model && !offered.some((m) => m.value === model)
      ? [...offered, { value: model, name: model }]
      : offered;
  return (
    <div className="row even-fields" style={{ marginTop: 10, gap: 12, alignItems: "flex-start" }}>
      <Field label="Agent" hint={agentHint}>
        <select className="ui-field" value={agentId} onChange={(e) => onChange(e.target.value, "")}>
          {!conversational.some((a) => a.id === agentId) && (
            <option value={agentId}>{agentName}</option>
          )}
          {conversational.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Model"
        hint={
          offered.length === 0
            ? `Open a chat with ${agentName} once and its models are listed here.`
            : modelHint
        }
      >
        <select
          className="ui-field"
          value={model}
          onChange={(e) => onChange(agentId, e.target.value)}
        >
          <option value="">{agentName} decides</option>
          {models.map((m) => (
            <option key={m.value} value={m.value}>
              {m.name}
            </option>
          ))}
        </select>
      </Field>
      {children}
    </div>
  );
}
