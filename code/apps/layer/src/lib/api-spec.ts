import { invoke } from "@tauri-apps/api/core";
import type {
  AppliedAnswers,
  Drafted,
  InstallState,
  RepoResult,
  SpecCheck,
  SpecCommit,
  SpecKind,
  SpecPart,
  SpecView,
} from "./types";

/**
 * The spec's commands (§19), split from `api.ts` along their own seam and
 * spread into `api`, so a call is still `api.readSpec`. Like `api.ts`, the
 * one place besides it that imports `invoke`: scripts/guard.ts reads every
 * `api-*.ts` as part of the frontend's view of the backend.
 */
export const specApi = {
  /** The task's spec: the toolset's files with their steps, each repository's share, the install. */
  readSpec: (taskId: string) => invoke<SpecView>("read_spec", { taskId }),
  /** Replace one file of the spec, as an agent's `spec_write` does. */
  writeSpecFile: (taskId: string, path: string, text: string) =>
    invoke<void>("write_spec_file", { taskId, path, text }),
  /** Delete one file of the spec: Undo of a draft that made it, so no empty file is left. */
  removeSpecFile: (taskId: string, path: string) =>
    invoke<void>("remove_spec_file", { taskId, path }),
  /** Kiro's Draft: one part for the whole task, streamed as `spec:draft`, then written into its file. */
  draftSpec: (taskId: string, part: SpecPart, kind: SpecKind | null, requestId: string) =>
    invoke<Drafted>("draft_spec", { taskId, part, kind, requestId }),
  /** Kiro's Apply answers: the answered open questions built into the requirements file (SPEC-26). */
  applySpecAnswers: (taskId: string, requestId: string) =>
    invoke<AppliedAnswers>("apply_spec_answers", { taskId, requestId }),
  /** Kiro's Check against spec, for one repository (SPEC-27). */
  checkSpec: (taskId: string, checkoutId: string) =>
    invoke<SpecCheck>("check_spec", { taskId, checkoutId }),
  /** Install the task's spec toolset again, after a failure; announced as `spec:install`. */
  installSpecToolset: (taskId: string) => invoke<InstallState>("install_spec_toolset", { taskId }),
  watchSpec: (taskId: string | null) => invoke<void>("watch_spec", { taskId }),
  /**
   * Commit spec: put the task's `spec/` into each repository per its
   * `spec_commit` and commit it there. `repos` limits it to those folders.
   */
  commitSpec: (taskId: string, repos: string[] | null = null) =>
    invoke<RepoResult[]>("commit_spec_cmd", { taskId, repos }),
  /** The toolset a task gets when nothing else says; null is Kiro. */
  setDefaultToolset: (id: string | null) => invoke<void>("set_default_toolset", { id }),
  /** Replace the user's own toolsets with the manifests in `json`; refused whole when one is not a manifest. */
  setCustomToolsets: (json: string) => invoke<void>("set_custom_toolsets", { json }),
  setProjectSpecs: (
    projectId: string,
    toolset: string | null,
    commit: SpecCommit,
    folder: string,
  ) => invoke<void>("set_project_specs", { projectId, toolset, commit, folder }),
};
