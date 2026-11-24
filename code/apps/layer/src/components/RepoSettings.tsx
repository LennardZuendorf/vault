import { useEffect, useState } from "react";
import { updateByFor } from "../lib/derive";
import type { Project, RepoHealth, SpecCommit, UpdateBy } from "../lib/types";
import { useStore } from "../store";

// A repository's own settings, as fields on its row in the Repos view.

/**
 * How Update from base updates branches here (UPD-7). Unset, it follows
 * the guess from history, and the first option says what that is.
 */
export function UpdateByPicker({
  p,
  h,
  onChange,
}: {
  p: Project;
  h: RepoHealth | undefined;
  onChange: (by: UpdateBy | null) => void;
}) {
  const guess = updateByFor(undefined, h);
  return (
    <select
      className="ui-field update-by"
      value={p.update_by ?? ""}
      title={`How Update from base brings the base in. ${updateByFor(p, h).why}`}
      onChange={(e) => onChange((e.target.value || null) as UpdateBy | null)}
    >
      <option value="">{h?.update_guess ? `${guess.by}s (guessed)` : "merges (default)"}</option>
      <option value="merge">merges</option>
      <option value="rebase">rebases</option>
    </select>
  );
}

/**
 * What a loop runs here (LOOP-1), saved on Enter or leaving the field. One
 * the workspace sent is confirmed by Enter on it: leaving the field is not
 * enough, since a loop runs it as you, unattended.
 */
export function CheckCommand({ p, onSave }: { p: Project; onSave: (command: string) => void }) {
  const [value, setValue] = useState(p.check ?? "");
  useEffect(() => setValue(p.check ?? ""), [p.check]);
  const unconfirmed = !!p.check?.trim() && p.check !== p.check_approved;
  return (
    <input
      className="ui-field repo-check mono"
      value={value}
      placeholder="check command, for loops"
      title={
        unconfirmed
          ? "This check came from the workspace and is not confirmed on this Mac, so loops don't run it. Press Enter here to confirm it."
          : "What says the work here is done (bun run check, cargo test). A loop runs it after each of an agent's turns."
      }
      spellCheck={false}
      onChange={(e) => setValue(e.target.value)}
      onBlur={() => {
        if (value.trim() !== (p.check ?? "")) onSave(value.trim());
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          if (unconfirmed && value.trim() === p.check) onSave(value.trim());
          e.currentTarget.blur();
        }
        if (e.key === "Escape") setValue(p.check ?? "");
      }}
    />
  );
}

/** What Commit spec puts in a repository (SPEC-12), as the person reads it. */
const COMMITS: { value: SpecCommit; label: string }[] = [
  { value: "whole", label: "commits the whole spec" },
  { value: "own", label: "commits its own part" },
  { value: "none", label: "kept by the app, not committed" },
];

/**
 * How a repository takes specs (§19): the toolset Start work offers for it,
 * what Commit spec puts in it, and, for Kiro, the folder (the other toolsets
 * mirror their own layout from the repository root). A toolset this Mac does
 * not have (a teammate's custom one) is kept, and said not to count.
 */
export function RepoSpecs({
  p,
  onChange,
}: {
  p: Project;
  onChange: (toolset: string | null, commit: SpecCommit, folder: string) => void;
}) {
  const settings = useStore((s) => s.settings);
  const choices = settings?.toolsets ?? [];
  const known = choices.some((t) => t.id === p.spec_toolset);
  const unknown = !!p.spec_toolset && !known;
  const effective = (known ? p.spec_toolset : null) ?? settings?.default_toolset ?? "kiro";
  const commit = p.spec_commit ?? "whole";
  const folder = p.spec_folder ?? "specs";
  const folders = [...new Set(["specs", "docs/specs", folder])];
  const set = (toolset: string | null, c: SpecCommit, f: string) => onChange(toolset, c, f);

  return (
    <div className="repo-sub repo-specs">
      <span className="muted">Specs</span>
      <select
        className="ui-field"
        aria-label="Spec toolset"
        value={p.spec_toolset ?? ""}
        title="The toolset Start work offers for this repository. A task keeps the one it was started with."
        onChange={(e) => set(e.target.value || null, commit, folder)}
      >
        <option value="">app default toolset</option>
        {choices.map((t) => (
          <option key={t.id} value={t.id}>
            {t.name}
          </option>
        ))}
        {unknown && (
          <option value={p.spec_toolset ?? ""}>{p.spec_toolset} (not on this Mac)</option>
        )}
      </select>
      <select
        className="ui-field"
        aria-label="Commit spec"
        value={commit}
        title="What Commit spec puts in this repository"
        onChange={(e) => set(p.spec_toolset ?? null, e.target.value as SpecCommit, folder)}
      >
        {COMMITS.map((c) => (
          <option key={c.value} value={c.value}>
            {c.label}
          </option>
        ))}
      </select>
      {effective === "kiro" && commit !== "none" && (
        <select
          className="ui-field"
          aria-label="Spec folder"
          value={folder}
          title="Where in the repository Kiro's spec is committed"
          onChange={(e) => set(p.spec_toolset ?? null, commit, e.target.value)}
        >
          {folders.map((f) => (
            <option key={f} value={f}>
              in {f}/
            </option>
          ))}
        </select>
      )}
      {unknown && (
        <span className="muted">
          {p.spec_toolset} is not a toolset this Mac has, so Start work ignores it here. Add it
          under Settings → Code.
        </span>
      )}
    </div>
  );
}
