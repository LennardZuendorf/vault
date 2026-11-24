import type { Edit } from "../../lib/specs";

/**
 * The files of the task's spec as a tree, the way the toolset lays them out
 * (Kiro's four at the top, OpenSpec's under `openspec/changes/<change>/`). A
 * folder is only a heading: it opens nothing, and a file is opened in the
 * editor beside it. A file with text not yet saved says so.
 */
export function SpecTree({
  paths,
  current,
  edits,
  onPick,
}: {
  paths: string[];
  current: string | null;
  edits: Record<string, Edit>;
  onPick: (path: string) => void;
}) {
  // A file may exist only in an editor, a draft being streamed into a new one.
  const folder = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");
  // By folder, then name, so a folder's files sit under one heading. Plain
  // code-unit order, whatever the locale: the heading text is the path.
  const all = [...new Set([...paths, ...Object.keys(edits)])].sort((a, b) => {
    const [x, y] = [folder(a), folder(b)];
    return x === y ? (a < b ? -1 : 1) : x < y ? -1 : 1;
  });
  // A heading for each folder that holds files, its whole path: a toolset's
  // folders are deep and mostly one name each (`docs/superpowers/plans/`).
  const rows: { key: string; dir?: string; path?: string }[] = [];
  let last = "";
  for (const path of all) {
    const dir = folder(path);
    if (dir !== last && dir !== "") rows.push({ key: `dir:${dir}`, dir });
    last = dir;
    rows.push({ key: path, path });
  }

  return (
    <nav className="spec-tree" aria-label="Spec files">
      {rows.map((r) =>
        r.path === undefined ? (
          <div key={r.key} className="spec-dir">
            {r.dir}/
          </div>
        ) : (
          <button
            key={r.key}
            className={`spec-file${r.path === current ? " active" : ""}`}
            aria-current={r.path === current ? "true" : undefined}
            title={r.path}
            onClick={() => onPick(r.path!)}
          >
            <span className="spec-file-name">{r.path.split("/").pop()}</span>
            {edits[r.path] && (
              <span
                className={`spec-file-mark${edits[r.path].conflict ? " conflict" : ""}`}
                title={edits[r.path].conflict ? "Changed on disk while you edited it" : "Not saved"}
                aria-label={edits[r.path].conflict ? "Changed on disk" : "Not saved"}
              >
                ●
              </span>
            )}
          </button>
        ),
      )}
    </nav>
  );
}
