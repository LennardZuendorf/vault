/** A task's spec (§19), mirrored from `commands/spec_view.rs`, `spec_draft.rs` and `spec_install.rs`. */

/** What Commit spec puts in a repository: all of it, its own part, or nothing. */
export type SpecCommit = "whole" | "own" | "none";

/** A toolset as a pick list offers it (`commands/spec_settings.rs`). `custom`: the user's own, on this Mac. */
export interface ToolsetChoice {
  id: string;
  name: string;
  version: string | null;
  custom: boolean;
}

/** The spec's part of the app's settings (§19), which `Settings` extends. */
export interface SpecSettings {
  /** Agents are told to write the spec before any code (SPEC-38). */
  spec_first: boolean;
  /** The toolset a task gets when nothing else says; null is Kiro. */
  default_toolset: string | null;
  /** Every toolset there is to pick from. */
  toolsets: ToolsetChoice[];
  /** The manifests the user added, shown as JSON in Settings → Code. */
  custom_toolsets: unknown[];
}

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
  /** Files the toolset keeps that are not UTF-8 text: not shown, and left out of Commit spec. */
  unreadable: string[];
  repos: RepoSpecView[];
  install: InstallState | null;
}

/** What a draft wrote (`after`), and what the file held before it (null: nothing), for Undo. */
export interface Drafted {
  path: string;
  before: string | null;
  after: string;
}

/** What Apply answers did to the requirements, by id, and what the file held before (SPEC-20). */
export interface AppliedAnswers {
  path: string;
  before: string;
  /** What it wrote: Undo puts `before` back only while the file holds this. */
  after: string;
  changed: string[];
  added: string[];
  removed: string[];
}
