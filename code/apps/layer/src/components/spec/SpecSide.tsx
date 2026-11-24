import type { ReactNode } from "react";
import { useSpecs } from "../../lib/specs";
import type { RepoResult, RepoSpecView, SpecView, TaskView } from "../../lib/types";
import { useStore } from "../../store";
import { Spinner } from "../ui";

const VERDICT_MARK: Record<string, string> = { met: "✓", "not met": "✗", unclear: "?" };

/**
 * Beside the editor: what the file being edited needs first (`children`: the
 * open questions of the requirements), and each repository's share of the
 * spec: how far its steps are (SPEC-14) and where Commit spec stands for it,
 * with what the last Commit spec did there. With Kiro's Check against spec,
 * each repository's last check.
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
  const committed = useSpecs((s) => s.byTask[task.id]?.committed ?? null);
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
            const complete = r.total > 0 && r.done === r.total;
            const result = committed?.find((c) => c.checkout_id === r.checkout_id);
            return (
              <li key={r.checkout_id} className={complete ? "done" : ""}>
                <span className="mark">{complete ? "✓" : "○"}</span>
                <div className="spec-repo-text">
                  <span>
                    {r.folder}: {r.total === 0 ? "no steps" : `${r.done} of ${r.total} done`}
                  </span>
                  <CommitLine r={r} result={result} />
                  {r.check && !busy && (
                    <span className={`muted${r.check.stale ? " spec-stale" : ""}`}>
                      {r.check.stale
                        ? "Checked before the latest commits"
                        : `Checked at ${r.check.sha.slice(0, 7)}`}
                      {": "}
                      {r.check.results.map((v) => `${v.id} ${VERDICT_MARK[v.verdict]}`).join(" ")}
                    </span>
                  )}
                </div>
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
              </li>
            );
          })}
        </ul>
      </section>
    </aside>
  );
}

/** Where Commit spec stands for one repository: what it did just now, or what it would do. */
function CommitLine({ r, result }: { r: RepoSpecView; result: RepoResult | undefined }) {
  if (r.commit === "none") return <span className="muted">Kept by the app, not committed.</span>;
  if (result && !result.ok) {
    return <span className="spec-failed">{sentence(`Not committed: ${result.detail}`)}</span>;
  }
  if (result) return <span className="muted">{sentence(result.detail)}</span>;
  return (
    <span className="muted">
      {r.changed_since_commit ? "Not in its last commit. Commit spec puts it there." : "Committed."}
    </span>
  );
}

function sentence(text: string): string {
  const capital = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(capital) ? capital : `${capital}.`;
}
