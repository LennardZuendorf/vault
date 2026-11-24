import type { ReactNode } from "react";
import { useSpecs } from "../../lib/specs";
import type { SpecView, TaskView } from "../../lib/types";
import { useStore } from "../../store";
import { Spinner } from "../ui";

const VERDICT_MARK: Record<string, string> = { met: "✓", "not met": "✗", unclear: "?" };

/**
 * Beside the editor: what the file being edited needs first (`children`: the
 * open questions of the requirements), how far each repository is through the
 * spec's steps (SPEC-8), and where Commit spec stands for it. With Kiro's
 * Check against spec, each repository's last check.
 */
export function SpecSide({
  task,
  spec,
  children,
}: {
  task: TaskView;
  spec: SpecView;
  children?: ReactNode;
}) {
  const checking = useSpecs((s) => s.byTask[task.id]?.checking ?? null);
  const check = useSpecs((s) => s.check);
  const fail = useStore((s) => s.fail);
  const checks = spec.toolset.extras.includes("check") && !task.epic;

  return (
    <aside className="spec-side">
      {children}
      <section>
        <h3>Repositories</h3>
        <ul className="spec-tasks">
          {spec.repos.map((r) => {
            const busy = checking?.includes(r.checkout_id) ?? false;
            return (
              <li key={r.checkout_id} className={r.total > 0 && r.done === r.total ? "done" : ""}>
                <span className="mark">{r.total > 0 && r.done === r.total ? "✓" : "○"}</span>
                <span>
                  {r.folder}: {r.total === 0 ? "no steps" : `${r.done} of ${r.total} done`}
                  {r.commit !== "none" && r.changed_since_commit && " · changed since committed"}
                  {r.commit === "none" && " · kept by the app"}
                </span>
                {checks && (
                  <button
                    className="ui-button"
                    disabled={busy}
                    title={`Read ${r.folder}'s changes on the branch against the requirements that name it`}
                    onClick={() => void check(task.id, r.checkout_id).catch(fail)}
                  >
                    Check
                  </button>
                )}
                {busy && <Spinner />}
                {r.check && !busy && (
                  <span className={`muted${r.check.stale ? " spec-stale" : ""}`}>
                    {r.check.stale
                      ? "Checked before the latest commits"
                      : `Checked at ${r.check.sha.slice(0, 7)}`}
                    {": "}
                    {r.check.results.map((v) => `${v.id} ${VERDICT_MARK[v.verdict]}`).join(" ")}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      </section>
    </aside>
  );
}
