/** A task's spec (§19), mirrored from `commands/spec_view.rs`, `spec_draft.rs` and `spec_install.rs`. */

/** What Commit spec puts in a repository: all of it, its own part, or nothing. */
export type SpecCommit = "whole" | "own" | "none";

/** One part of a Kiro spec, for Draft (SPEC-5). */
export type SpecPart = "requirements" | "design" | "tasks";
export type SpecKind = "feature" | "bugfix";

/**
 * How installing a task's spec toolset stands (`commands/spec_install.rs`):
 * what the Spec tab shows as a line and a dot. `running`: it is under way
 * now, and `ok` is not yet its answer.
 */
export interface InstallState {
  toolset: string;
  version: string | null;
  ok: boolean;
  error: string | null;
  running: boolean;
}

/** The toolset as the tab shows it. `extras`: what only some toolsets have (Kiro's three). */
export interface SpecToolset {
  id: string;
  name: string;
  version: string | null;
  extras: ("draft" | "check" | "questions")[];
}

/**
 * One `- [ ]` step. `position` is how it is addressed (1-based over every
 * checkbox of its file); `repos` are the folders that own it, none meaning
 * every repository.
 */
export interface SpecStep {
  position: number;
  label: string;
  text: string;
  done: boolean;
  repos: string[];
}

/** One file of the spec, with its steps when the toolset keeps steps in it. */
export interface SpecFileView {
  path: string;
  text: string;
  steps: SpecStep[];
}

export interface SpecVerdict {
  id: string;
  verdict: "met" | "not met" | "unclear";
  evidence: string;
}

/** The last check against the spec (SPEC-15). `stale`: the branch moved on since. */
export interface SpecCheck {
  sha: string;
  at: number;
  results: SpecVerdict[];
  stale: boolean;
}

/** One repository's share of the spec. */
export interface RepoSpecView {
  checkout_id: string;
  repo: string;
  folder: string;
  commit: SpecCommit;
  /** Steps ticked of all the repository owns, or that belong to everyone. */
  done: number;
  total: number;
  /** What Commit spec would write now is not what its last commit holds. */
  changed_since_commit: boolean;
  check: SpecCheck | null;
}

export interface SpecView {
  toolset: SpecToolset;
  /** The working copy, where agents and the user edit. */
  dir: string;
  files: SpecFileView[];
  repos: RepoSpecView[];
  install: InstallState | null;
}

/** What a draft wrote, and what the file held before it (null: nothing), for Undo. */
export interface Drafted {
  path: string;
  before: string | null;
}

/** What Apply answers did to the requirements, by id, and what the file held before (SPEC-20). */
export interface AppliedAnswers {
  path: string;
  before: string;
  changed: string[];
  added: string[];
  removed: string[];
}
