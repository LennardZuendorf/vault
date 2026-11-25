import { create } from "zustand";
import { api } from "./api";
import { mergeDisk } from "./spec-edit";
import type {
  AppliedAnswers,
  InstallState,
  RepoResult,
  SpecKind,
  SpecPart,
  SpecView,
} from "./types";

/**
 * What an editor holds where it is not what its file holds: the text, what the
 * file held when the editing began, and whether the file has changed since
 * (`conflict`: an agent wrote it, SPEC-37).
 */
export interface Edit {
  text: string;
  base: string;
  conflict: boolean;
}

/**
 * Each task's spec as the Spec tab holds it (§19). The spec is the files in
 * the task's `spec/` folder, which agents write too, so what is kept here is
 * what was read, and the text of an editor not yet saved. A draft is streamed
 * into the editor, and written into the file by the backend when it is done.
 */
export interface SpecState {
  loaded: boolean;
  spec: SpecView | null;
  /** What each editor holds where it is not what the file holds, by path. */
  edits: Record<string, Edit>;
  /** A Kiro draft or Apply answers being streamed in, and its request. */
  drafting: { requestId: string } | null;
  /** Repositories whose check is running (SPEC-15). */
  checking: string[];
  /** What the last draft or Apply answers replaced, for Undo (SPEC-20). */
  last: { path: string; before: string | null; results: AppliedAnswers | null } | null;
  /** What the last Commit spec did in each repository, until the spec changes again. */
  committed: RepoResult[] | null;
}

interface Specs {
  byTask: Record<string, SpecState>;
  /** How each task's toolset install stands, as last announced (`spec:install`). */
  install: Record<string, InstallState>;
  installed: (taskId: string, state: InstallState) => void;
  /** Install the toolset again, for a task whose install failed. */
  retryInstall: (taskId: string) => Promise<void>;
  /** Commit spec: a row per repository, a failed one named with why. */
  commitSpec: (taskId: string) => Promise<void>;
  load: (taskId: string) => Promise<void>;
  edit: (taskId: string, path: string, text: string) => void;
  /**
   * Write what an editor holds into its file. False when an agent wrote the file
   * since the editing began: nothing is written, the text is kept and marked a
   * conflict. `force` (Save mine) writes over it (SPEC-37).
   */
  save: (taskId: string, path: string, force?: boolean) => Promise<boolean>;
  /** Let go of what an editor holds: the file's text is shown again (Load theirs). */
  discard: (taskId: string, path: string) => void;
  /** Kiro's Draft: one part of the spec for the whole task. */
  draft: (taskId: string, part: SpecPart, kind: SpecKind | null) => Promise<void>;
  check: (taskId: string, checkoutId: string) => Promise<void>;
  /** Kiro's Apply answers: the answered open questions built into the requirements. */
  applyAnswers: (taskId: string) => Promise<void>;
  /** Write back what the last draft or Apply answers replaced. */
  undo: (taskId: string) => Promise<void>;
  /** A piece of a draft under way (`spec:draft`): its file's whole text so far. */
  chunk: (requestId: string, path: string, text: string) => void;
  /** An agent wrote or ticked, or one stopped (`spec:changed`). */
  changed: (taskId: string) => void;
}

/** The file a Kiro spec's requirements are in: a bug's analysis, or else the requirements. */
export function requirementsPath(spec: SpecView | null | undefined): string | null {
  const has = (p: string) => spec?.files.some((f) => f.path === p) ?? false;
  return has("bugfix.md") ? "bugfix.md" : has("requirements.md") ? "requirements.md" : null;
}

/** What an editor shows: the edit, else the file. */
export function editorText(state: SpecState | undefined, path: string): string {
  return state?.edits[path]?.text ?? state?.spec?.files.find((f) => f.path === path)?.text ?? "";
}

/** The tab's badge: steps done of all, or else how many files there are. */
export function specProgress(spec: SpecView | null): string | null {
  const total = (spec?.repos ?? []).reduce((n, r) => n + r.total, 0);
  const done = (spec?.repos ?? []).reduce((n, r) => n + r.done, 0);
  if (total > 0) return `${done}/${total}`;
  const files = spec?.files.length ?? 0;
  return files > 0 ? String(files) : null;
}

const EMPTY: SpecState = {
  loaded: false,
  spec: null,
  edits: {},
  drafting: null,
  checking: [],
  last: null,
  committed: null,
};

export const useSpecs = create<Specs>((set, get) => {
  const put = (taskId: string, part: Partial<SpecState>) =>
    set((s) => ({
      byTask: { ...s.byTask, [taskId]: { ...(s.byTask[taskId] ?? EMPTY), ...part } },
    }));
  const state = (taskId: string) => get().byTask[taskId] ?? EMPTY;
  const without = (edits: Record<string, Edit>, path: string) => {
    const rest = { ...edits };
    delete rest[path];
    return rest;
  };
  /** Reads started and answered in order, so a slow answer cannot put back an older spec. */
  const reads = new Map<string, number>();

  /** A run that writes into a file, its text streamed into the editor first. */
  async function streamed(taskId: string, run: (requestId: string) => Promise<string>) {
    const before = state(taskId).edits;
    const requestId = `spec-${crypto.randomUUID()}`;
    put(taskId, { drafting: { requestId }, last: null });
    try {
      const path = await run(requestId);
      // The file holds it now.
      put(taskId, { drafting: null, edits: without(state(taskId).edits, path) });
      await get().load(taskId);
    } catch (e) {
      put(taskId, { drafting: null, edits: before });
      throw e;
    }
  }

  return {
    byTask: {},
    install: {},
    installed: (taskId, installState) => {
      set((s) => ({ install: { ...s.install, [taskId]: installState } }));
      // A finished install may have written files into the spec.
      if (!installState.running) get().changed(taskId);
    },
    retryInstall: async (taskId) => {
      get().installed(taskId, await api.installSpecToolset(taskId));
    },
    commitSpec: async (taskId) => {
      const committed = await api.commitSpec(taskId);
      put(taskId, { committed });
      await get().load(taskId);
    },
    load: async (taskId) => {
      const mine = (reads.get(taskId) ?? 0) + 1;
      reads.set(taskId, mine);
      const spec = await api.readSpec(taskId);
      if (reads.get(taskId) !== mine) return;
      const now = state(taskId);
      // A draft under way writes into the editors itself.
      const edits = now.drafting ? now.edits : reconcile(now.edits, spec);
      put(taskId, { loaded: true, spec, edits });
    },
    edit: (taskId, path, text) => {
      const now = state(taskId);
      const disk = now.spec?.files.find((f) => f.path === path)?.text;
      const edits =
        disk === text
          ? without(now.edits, path)
          : {
              ...now.edits,
              [path]: {
                text,
                base: now.edits[path]?.base ?? disk ?? "",
                conflict: now.edits[path]?.conflict ?? false,
              },
            };
      put(taskId, { edits, last: null });
    },
    save: async (taskId, path, force = false) => {
      const edit = state(taskId).edits[path];
      if (!edit) return true;
      // The backend checks `base` under the lock every write takes, so an agent's
      // write between the last read and this save is not overwritten unseen.
      const written = await api.writeSpecFile(taskId, path, edit.text, edit.base, force);
      if (!written) {
        put(taskId, { edits: { ...state(taskId).edits, [path]: { ...edit, conflict: true } } });
        await get().load(taskId);
        return false;
      }
      put(taskId, { edits: without(state(taskId).edits, path) });
      await get().load(taskId);
      return true;
    },
    discard: (taskId, path) => put(taskId, { edits: without(state(taskId).edits, path) }),
    draft: (taskId, part, kind) =>
      streamed(taskId, async (requestId) => {
        const done = await api.draftSpec(taskId, part, kind, requestId);
        put(taskId, { last: { path: done.path, before: done.before, results: null } });
        return done.path;
      }),
    check: async (taskId, checkoutId) => {
      put(taskId, { checking: [...state(taskId).checking, checkoutId] });
      try {
        await api.checkSpec(taskId, checkoutId);
        await get().load(taskId);
      } finally {
        put(taskId, { checking: state(taskId).checking.filter((c) => c !== checkoutId) });
      }
    },
    applyAnswers: async (taskId) => {
      // The run reads the file, so what the editor holds goes into it first.
      const path = requirementsPath(state(taskId).spec);
      if (path && !(await get().save(taskId, path))) {
        throw new Error(`${path} changed on disk. Load theirs or Save mine first.`);
      }
      await streamed(taskId, async (requestId) => {
        const done = await api.applySpecAnswers(taskId, requestId);
        put(taskId, { last: { path: done.path, before: done.before, results: done } });
        return done.path;
      });
    },
    undo: async (taskId) => {
      const last = state(taskId).last;
      if (!last) return;
      // A draft that made the file leaves none behind, not an empty one.
      if (last.before === null) await api.removeSpecFile(taskId, last.path);
      else await api.writeSpecFile(taskId, last.path, last.before);
      put(taskId, { last: null, edits: without(state(taskId).edits, last.path) });
      await get().load(taskId);
    },
    chunk: (requestId, path, text) => {
      const entry = Object.entries(get().byTask).find(
        ([, s]) => s.drafting?.requestId === requestId,
      );
      if (!entry) return;
      const [taskId, s] = entry;
      const disk = s.spec?.files.find((f) => f.path === path)?.text ?? "";
      put(taskId, { edits: { ...s.edits, [path]: { text, base: disk, conflict: false } } });
    },
    changed: (taskId) => {
      if (state(taskId).loaded) {
        put(taskId, { committed: null });
        void get()
          .load(taskId)
          .catch(() => {});
      }
    },
  };
});

/**
 * The editors once the files are read again: each keeps its text, and says so
 * when its file is no longer what the editing began from (`mergeDisk`). An
 * editor whose text is now the file's has nothing left to keep. Unchanged
 * edits are the same objects, so nothing re-renders for them.
 */
export function reconcile(edits: Record<string, Edit>, spec: SpecView): Record<string, Edit> {
  let next = edits;
  for (const [path, edit] of Object.entries(edits)) {
    const disk = spec.files.find((f) => f.path === path)?.text ?? "";
    if (disk === edit.text) {
      next = { ...next };
      delete next[path];
      continue;
    }
    const { conflict } = mergeDisk({ text: edit.text, dirty: true, base: edit.base }, disk);
    if (conflict !== edit.conflict) next = { ...next, [path]: { ...edit, conflict } };
  }
  return next;
}
