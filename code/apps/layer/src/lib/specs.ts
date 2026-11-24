import { create } from "zustand";
import { api } from "./api";
import type {
  AppliedAnswers,
  InstallState,
  RepoResult,
  SpecKind,
  SpecPart,
  SpecView,
} from "./types";

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
  edits: Record<string, string>;
  /** A Kiro draft or Apply answers being streamed in, and its request. */
  drafting: { requestId: string } | null;
  /** Repositories whose check is running (SPEC-15). */
  checking: string[];
  /** What the last draft or Apply answers replaced, for Undo (SPEC-20). */
  last: { path: string; before: string | null; results: AppliedAnswers | null } | null;
}

interface Specs {
  byTask: Record<string, SpecState>;
  /** How each task's toolset install stands, as last announced (`spec:install`). */
  install: Record<string, InstallState>;
  installed: (taskId: string, state: InstallState) => void;
  /** Install the toolset again, for a task whose install failed. */
  retryInstall: (taskId: string) => Promise<void>;
  /** Commit spec: a row per repository, a failed one named with why. */
  commitSpec: (taskId: string, repos?: string[]) => Promise<RepoResult[]>;
  load: (taskId: string) => Promise<void>;
  edit: (taskId: string, path: string, text: string) => void;
  /** Write what an editor holds into its file. */
  save: (taskId: string, path: string) => Promise<void>;
  /** Let go of what an editor holds. */
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
  /** The agent Start work picked, until Task 10's Start work rewires it (SPEC-18). */
  pending: { taskId: string; agentId: string | null } | null;
  setPending: (p: { taskId: string; agentId: string | null } | null) => void;
}

/** The file a Kiro spec's requirements are in: a bug's analysis, or else the requirements. */
export function requirementsPath(spec: SpecView | null | undefined): string | null {
  const has = (p: string) => spec?.files.some((f) => f.path === p) ?? false;
  return has("bugfix.md") ? "bugfix.md" : has("requirements.md") ? "requirements.md" : null;
}

/** What an editor shows: the edit, else the file. */
export function editorText(state: SpecState | undefined, path: string): string {
  return state?.edits[path] ?? state?.spec?.files.find((f) => f.path === path)?.text ?? "";
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
};

export const useSpecs = create<Specs>((set, get) => {
  const put = (taskId: string, part: Partial<SpecState>) =>
    set((s) => ({
      byTask: { ...s.byTask, [taskId]: { ...(s.byTask[taskId] ?? EMPTY), ...part } },
    }));
  const state = (taskId: string) => get().byTask[taskId] ?? EMPTY;
  const without = (edits: Record<string, string>, path: string) => {
    const rest = { ...edits };
    delete rest[path];
    return rest;
  };

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
    installed: (taskId, state) => set((s) => ({ install: { ...s.install, [taskId]: state } })),
    retryInstall: async (taskId) => {
      get().installed(taskId, await api.installSpecToolset(taskId));
    },
    commitSpec: (taskId, repos) => api.commitSpec(taskId, repos ?? null),
    load: async (taskId) => {
      const spec = await api.readSpec(taskId);
      put(taskId, { loaded: true, spec });
    },
    edit: (taskId, path, text) => {
      const file = state(taskId).spec?.files.find((f) => f.path === path);
      const same = file !== undefined && file.text === text;
      put(taskId, {
        edits: same ? without(state(taskId).edits, path) : { ...state(taskId).edits, [path]: text },
        last: null,
      });
    },
    save: async (taskId, path) => {
      const text = state(taskId).edits[path];
      if (text === undefined) return;
      await api.writeSpecFile(taskId, path, text);
      put(taskId, { edits: without(state(taskId).edits, path) });
      await get().load(taskId);
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
      if (path) await get().save(taskId, path);
      await streamed(taskId, async (requestId) => {
        const done = await api.applySpecAnswers(taskId, requestId);
        put(taskId, { last: { path: done.path, before: done.before, results: done } });
        return done.path;
      });
    },
    undo: async (taskId) => {
      const last = state(taskId).last;
      if (!last) return;
      await api.writeSpecFile(taskId, last.path, last.before ?? "");
      put(taskId, { last: null, edits: without(state(taskId).edits, last.path) });
      await get().load(taskId);
    },
    chunk: (requestId, path, text) => {
      const entry = Object.entries(get().byTask).find(
        ([, s]) => s.drafting?.requestId === requestId,
      );
      if (!entry) return;
      const [taskId, s] = entry;
      put(taskId, { edits: { ...s.edits, [path]: text } });
    },
    changed: (taskId) => {
      if (state(taskId).loaded)
        void get()
          .load(taskId)
          .catch(() => {});
    },
    pending: null,
    setPending: (pending) => set({ pending }),
  };
});
