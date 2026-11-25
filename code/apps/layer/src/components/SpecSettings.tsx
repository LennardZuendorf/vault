import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useStore } from "../store";
import { ErrorMessage, Field, Switch, useFailure } from "./ui";

/**
 * Settings → Code, for specs (§19): the toolset a task gets by default, Spec
 * first (SPEC-38), and the user's own toolsets as the JSON of their manifests.
 * Each saves where it is changed: the toolsets, as a whole, when the field is
 * left, since a list that is half a manifest means nothing.
 */
export function SpecSettings() {
  const settings = useStore((s) => s.settings);
  const refreshSettings = useStore((s) => s.refreshSettings);
  const setSpecFirst = useStore((s) => s.setSpecFirst);
  const failure = useFailure();
  const custom = settings?.custom_toolsets ?? [];
  const shown = custom.length > 0 ? JSON.stringify(custom, null, 2) : "";
  const [json, setJson] = useState(shown);
  // The field follows what is stored, and only when that changes.
  useEffect(() => setJson(shown), [shown]);
  if (!settings) return null;
  const choices = settings.toolsets;
  const current = settings.default_toolset ?? "kiro";

  async function save(what: () => Promise<unknown>) {
    failure.clear();
    try {
      await what();
      await refreshSettings();
    } catch (e) {
      failure.fail(e);
    }
  }

  return (
    <div className="card" style={{ margin: "14px 0" }}>
      <h3>Specs</h3>
      <Field
        label="Default toolset"
        hint="What a new task writes its spec with, unless Start work or the repository says otherwise. A task keeps the toolset it was made with."
      >
        <select
          className="ui-field"
          value={current}
          onChange={(e) => void save(() => api.setDefaultToolset(e.target.value))}
        >
          {choices.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name}
              {t.version ? ` ${t.version}` : ""}
            </option>
          ))}
          {!choices.some((t) => t.id === current) && (
            <option value={current}>{current} (no longer here)</option>
          )}
        </select>
      </Field>
      <div className="switch-list" style={{ marginBottom: 14 }}>
        <Switch
          label="Always write a spec first"
          detail="Every agent Layer starts is told to write or update the spec, then stop until you say go. Sub-tasks work from their epic's spec and are not told. Instructions only: nothing stops an agent that ignores them."
          checked={settings.spec_first}
          onChange={(on) => void save(() => setSpecFirst(on))}
        />
      </div>
      <Field
        label="Custom toolsets"
        hint="A JSON list of manifests: id, name, files, steps, install. One with a built-in's id replaces it. Kept on this Mac only, never shared with the workspace."
      >
        <textarea
          className="ui-field mono"
          rows={json ? 10 : 4}
          value={json}
          spellCheck={false}
          placeholder='[{ "id": "mine", "name": "Mine", "files": ["spec/*.md"], "steps": ["spec/plan.md"] }]'
          onChange={(e) => setJson(e.target.value)}
          onBlur={() => {
            if (json.trim() !== shown.trim()) void save(() => api.setCustomToolsets(json));
          }}
        />
        <p className="ui-tint-warn error-message">
          Install steps that <code>run</code> execute as written, on this Mac, as you. Add only
          manifests you trust.
        </p>
      </Field>
      <ErrorMessage>{failure.error}</ErrorMessage>
    </div>
  );
}
