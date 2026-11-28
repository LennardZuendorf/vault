/**
 * The other two toolsets' specs for the mock harness (§19): what OpenSpec and
 * superpowers keep in a task's `spec/` for ACME-123's two repositories, laid
 * out as their manifests say (`src-tauri/toolsets/*.json`). `?toolset=openspec`
 * or `?toolset=superpowers` gives ACME-123 one of them; Kiro's files are in
 * `spec.ts`.
 */
import type { SpecToolset } from "../lib/types";

export interface ToolsetWorld {
  toolset: SpecToolset;
  /** Which files hold `- [ ]` steps. */
  holdsSteps: (path: string) => boolean;
  files: Record<string, string>;
}

const CHANGE = "openspec/changes/fix-login-race";

const OPENSPEC: ToolsetWorld = {
  toolset: { id: "openspec", name: "OpenSpec", version: "1.14.3", extras: [] },
  holdsSteps: (path) => /^openspec\/changes\/[^/]+\/tasks\.md$/.test(path),
  files: {
    [`${CHANGE}/proposal.md`]: `## Why
Two sign-ins for one account at once leave one of them signed out.

## What Changes
- Refresh sessions under a per-account lock.
- Say "Signed in elsewhere at the same moment. Try again." instead of looping.

## Impact
- api: \`src/auth/session.ts\`, \`src/auth/login.ts\`
- web: \`src/signin/Form.tsx\`
`,
    [`${CHANGE}/design.md`]: `## Context
Refresh is not atomic: two refreshes read the same token and the second write wins.

## Decisions
- A Redis lock per account around refresh, 2s at most. A lock not taken in 2s is a lost race.
`,
    [`${CHANGE}/tasks.md`]: `## 1. Session refresh [api]
- [x] 1.1 Take a per-account lock around session refresh
- [x] 1.2 Test two parallel sign-ins
- [ ] 1.3 Retry a lost race once, then answer 409

## 2. Sign-in form [web]
- [ ] 2.1 Disable submit while a sign-in is in flight
- [ ] 2.2 Show the message on a session_race 409
- [ ] 2.3 Test both in Form.test.tsx
`,
    [`${CHANGE}/specs/auth/spec.md`]: `## ADDED Requirements
### Requirement: Concurrent sign-ins
The system SHALL keep both sessions valid when two sign-ins for one account arrive within a second.

#### Scenario: Two sign-ins at once
- **WHEN** two sign-ins for one account arrive within a second
- **THEN** both sessions are valid
`,
  },
};

const SUPERPOWERS: ToolsetWorld = {
  toolset: { id: "superpowers", name: "superpowers", version: "7.0.2", extras: [] },
  holdsSteps: (path) => /^docs\/superpowers\/plans\/[^/]+\.md$/.test(path),
  files: {
    "docs/superpowers/specs/2026-11-24-login-race-design.md": `# Login race design

## Goal
Two sign-ins for one account at once keep both sessions.

## Approach
Refresh under a per-account lock in Redis (api); the web says so when the API gives up.

## Out of scope
Rate-limiting sign-ins.
`,
    "docs/superpowers/plans/2026-11-24-login-race.md": `# Login race Implementation Plan

### Task 1: Lock the refresh [api]

- [x] **Step 1: Write the failing test** for two parallel sign-ins
- [x] **Step 2: Take \`lock:session:<account>\` around refresh**
- [ ] **Step 3: Retry a lost race once, then answer 409**

### Task 2: Say so in the form [web]

- [ ] **Step 1: Disable submit while a sign-in is in flight**
- [ ] **Step 2: Show the message on a session_race 409**
`,
  },
};

export const WORLDS: Record<string, ToolsetWorld> = {
  openspec: OPENSPEC,
  superpowers: SUPERPOWERS,
};
