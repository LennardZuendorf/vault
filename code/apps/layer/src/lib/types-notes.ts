// Repo notes: what agents and the user wrote down about a repository
// (MEM-1), mirrored from `notes.rs`, and re-exported from `types.ts`.

/** A repo note (MEM-1), and whether it may be out of date (MEM-3). */
export interface RepoNote {
  id: string;
  /** The repository, as where it fetches from, spelled one way. */
  repo: string;
  text: string;
  /** Files or folders it is about, relative to the repository's root. */
  paths: string[];
  /** Unix milliseconds. */
  written_at: number;
  /** "Claude Code in ACME-12 …", or "you". */
  source: string;
  written_commit: string | null;
  /** Unix milliseconds. Writing and editing count as checking. */
  checked_at: number;
  checked_commit: string | null;
  /** Files under its paths changed on the default branch since it was last
   *  checked. null: that cannot be told (it names no paths, say). */
  changed: string[] | null;
}

export interface ProjectNotes {
  project_id: string;
  notes: RepoNote[];
}
