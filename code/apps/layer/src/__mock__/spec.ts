/**
 * Specs (§19) for the mock harness: the files of a spec in each task's
 * `spec/` folder, held in memory. ACME-123 has a spec under way for api and
 * web, with a check: Kiro's by default, OpenSpec's or superpowers' with
 * `?toolset=` (`spec-worlds.ts`), and `?install=failed|running` shows the
 * install it would be made with. ACME-130 has requirements drafted and half
 * answered (SPEC-19). Draft streams a file in a line at a time, as
 * `spec:draft`; Apply answers and Check answer from the requirements.
 * `__mock.spec.write(task, path, text)` is an agent writing a file. Parsing is
 * rough, as far as the seeded texts need; `spec/owned.rs` is the real thing.
 */
import { emit } from "@tauri-apps/api/event";
import { questions } from "../lib/questions";
import type {
  InstallState,
  RepoResult,
  RepoSpecView,
  SpecCommit,
  SpecPart,
  SpecStep,
  SpecVerdict,
  SpecView,
} from "../lib/types";
import { WORLDS, type ToolsetWorld } from "./spec-worlds";
import type { World } from "./world";

type Check = { sha: string; at: number; results: SpecVerdict[]; stale: boolean };

const API_REQUIREMENTS = `## Goal
Two logins at once no longer leave one of them signed out.

## Requirements
- R-1 [api]: WHEN two sign-ins for one account arrive within a second THE SYSTEM SHALL keep both sessions valid.
- R-2 [api]: WHEN a session refresh loses the race THE SYSTEM SHALL retry once, then answer 409 with a reason.
- R-3 [api, web]: WHEN a sign-in lost the race THE SYSTEM SHALL say "Signed in elsewhere at the same moment. Try again." instead of looping.
- R-4 [web]: WHEN a sign-in is in flight THE SYSTEM SHALL disable the submit button.

## Out of scope
- Rate-limiting sign-ins.

## Open questions
None.
`;

const API_DESIGN = `## Approach
Refresh under a per-account lock in Redis, so the second refresh waits for the first (R-1); the web says so when the API gives up (R-3).

## Contracts
- C-1: \`POST /sessions\` answers 409 with \`{ "reason": "session_race" }\` when a refresh lost the race twice. Provided by \`api\`, used by \`web\` (R-2, R-3).

## api
### Changes
- \`src/auth/session.ts\`: take \`lock:session:<account>\` around refresh (R-1).
- \`src/auth/login.ts\`: retry once on a lost race, then C-1 (R-2).
### Error handling
A lock not taken in 2s is a lost race.
### Testing
Two parallel sign-ins in \`session.test.ts\` (R-1); a forced lost race (R-2).

## web
### Changes
- \`src/signin/Form.tsx\`: disable submit while in flight (R-4); show the message on C-1 (R-3).
### Error handling
Any other 409 shows the generic error.
### Testing
\`Form.test.tsx\`: C-1 shows the message (R-3); double submit is blocked (R-4).
`;

const API_TASKS = `## Tasks
- [x] 1. Take a per-account lock around session refresh (R-1)
- [x] 2. Test two parallel sign-ins (R-1)
- [ ] 3. Retry a lost race once, then answer 409 (R-2, C-1)
- [ ] 4. Test a forced lost race (R-2)
`;

const WEB_TASKS = `## Tasks
- [ ] 1. Disable submit while a sign-in is in flight (R-4)
- [ ] 2. Show the message on a session_race 409 (R-3, C-1; after api 3)
- [ ] 3. Test both in Form.test.tsx (R-3, R-4)
`;

/** Both repositories' steps in one file, each under its own first-level heading. */
const TASKS = `# api\n\n${API_TASKS.trimEnd()}\n\n# web\n\n${WEB_TASKS.trimEnd()}\n`;

/** What a draft streams in, by file. */
const DRAFT: Record<SpecPart, (folder: string) => string> = {
  requirements: (folder) => `## Goal
Password resets are rate-limited, so one address cannot be flooded with reset mails.

## Requirements
- R-1 [${folder}]: WHEN a sixth reset is asked for one address within an hour THE SYSTEM SHALL refuse it with a clear message.
- R-2 [${folder}]: WHEN resets are asked for different addresses THE SYSTEM SHALL count them separately.

## Out of scope
- Rate-limiting sign-ins.

## Open questions
- Q-1: Should support staff be able to lift the limit for an address?
  - Yes, from the admin page
  - No
- Q-2: Should the refusal say when to try again?
  - Yes, the minutes left
  - No, only that it was refused
`,
  design: (folder) => `## Approach
Count reset requests per address in Redis with a one-hour expiry, in \`${folder}\`.

## Changes
- \`src/auth/reset.ts\`: check the count before sending (R-1, R-2).

## Interfaces
None.

## Error handling
Redis down: let the reset through, and log it.

## Testing
Six requests for one address (R-1); two addresses (R-2).
`,
  tasks: () => `## Tasks
- [ ] 1. Count resets per address with an hour's expiry (R-1, R-2)
- [ ] 2. Refuse the sixth with a clear message (R-1)
- [ ] 3. Test one address and two (R-1, R-2)
`,
};

/** ACME-130's requirements, drafted and half answered (SPEC-19). */
const AUDIT_DRAFT = `## Goal
Admins can see who changed what in the workspace, and when.

## Requirements
- R-1: WHEN an admin opens the audit log THE SYSTEM SHALL list changes newest first, with who, what and when.
- R-2: WHEN the list is filtered by person THE SYSTEM SHALL show only that person's changes.
- R-3: WHEN an entry is older than the kept period THE SYSTEM SHALL leave it out.

## Out of scope
- Exporting the log.

## Open questions
- Q-1: How long are entries kept?
  - 90 days
  - A year
  - For good
  Answer: A year
- Q-2: Can admins see changes made by other admins?
  - Yes
  - Only their own
- Q-3: Should the page load more as it scrolls, or in pages of 50?
`;

/**
 * What Apply answers does here (SPEC-20), roughly: each answered question
 * becomes a requirement and leaves the list. The real run rewrites the ones
 * it decides instead.
 */
function applied(text: string): string {
  const qs = questions(text);
  const head = text.split(/^## Open questions/m)[0].trimEnd();
  const ids = requirements(text).map((r) => Number(r.id.slice(2)));
  let next = Math.max(0, ...ids);
  const added = qs
    .filter((q) => q.answer)
    .map(
      (q) =>
        `- R-${++next}: WHEN ${q.text.replace(/\?$/, "").replace(/^\w/, (c) => c.toLowerCase())} THE SYSTEM SHALL do as decided: ${q.answer}.`,
    );
  const withReqs = head.replace(
    /(^- R-\d+[^\n]*\n)(?![\s\S]*^- R-\d+)/m,
    (m) => m + added.map((a) => a + "\n").join(""),
  );
  const left = qs
    .filter((q) => !q.answer)
    .map((q) => [`- ${q.id}: ${q.text}`, ...q.options.map((o) => `  - ${o}`)].join("\n"));
  return `${withReqs}\n\n## Open questions\n${left.length > 0 ? left.join("\n") : "None."}\n`;
}

function requirements(text: string) {
  const at = text.search(/^## (Requirements|Expected behaviour)/m);
  if (at < 0) return [];
  const part = text.slice(at).split(/^## (?!Unchanged)/m)[1] ?? "";
  return [...part.matchAll(/^- (?:\[[ xX]\] )?(R-\d+)(?: \[([^\]]*)\])?[:.]? (.+)$/gm)].map(
    (m) => ({
      id: m[1],
      repos: (m[2] ?? "")
        .split(",")
        .map((r) => r.trim())
        .filter(Boolean),
      text: m[3].trim(),
    }),
  );
}

/**
 * A step file's steps, each with the repository that owns it: the nearest
 * heading above (any level) that is exactly a folder's name or tags one
 * (`## 2. API [api]`), or a `[folder]` tag on the step's own line.
 */
function steps(text: string, folders: string[]): SpecStep[] {
  let owner: string | null = null;
  const found: SpecStep[] = [];
  const tagged = (line: string) => folders.find((f) => line.includes(`[${f}]`)) ?? null;
  for (const line of text.split("\n")) {
    const heading = line.match(/^#{1,6}\s+(.+)$/)?.[1].trim();
    if (heading) {
      const named = folders.find((f) => f === heading) ?? tagged(heading);
      if (named) owner = named;
    }
    const m = line.match(/^\s*- \[([ xX])\] (?:(\d+(?:\.\d+)*)\.? )?(.+)$/);
    if (m) {
      const own = tagged(m[3]) ?? owner;
      found.push({
        position: found.length + 1,
        label: m[2] ?? "",
        text: m[3].trim(),
        done: m[1] !== " ",
        repos: own ? [own] : [],
      });
    }
  }
  return found;
}

/** Streams `text` into an editor as `spec:draft`, a line at a time. */
async function stream(requestId: unknown, path: string, text: string, pause = 40) {
  let sent = "";
  for (const piece of text.match(/[^\n]*\n?/g) ?? []) {
    await new Promise((r) => setTimeout(r, pause));
    sent += piece;
    void emit("spec:draft", { request_id: requestId, path, text: sent });
  }
  return sent;
}

/** An agent writing a file of a task's spec, for the console: the tab follows it (SPEC-36). */
export const specDisk = {
  /** Repositories whose Commit spec fails, by folder (`["web"]`); the others keep theirs. */
  commitFails: [] as string[],
  write: (_taskId: string, _path: string, _text: string): void => {},
  remove: (_taskId: string, _path: string): void => {},
};

const KIRO: ToolsetWorld = {
  toolset: {
    id: "kiro",
    name: "Kiro (Layer)",
    version: null,
    extras: ["draft", "check", "questions"],
  },
  holdsSteps: (path) => path === "tasks.md",
  files: {},
};

export function specAnswers(world: World): Record<string, (a: Record<string, unknown>) => unknown> {
  const params = new URLSearchParams(location.search);
  const kept = new Map<string, Record<string, string>>();
  const installs = new Map<string, InstallState>();
  // ACME-123's toolset; every other task is Kiro's.
  const worldOf = (taskId: string): ToolsetWorld =>
    (taskId === "t-login" ? WORLDS[params.get("toolset") ?? ""] : undefined) ?? KIRO;
  const checks = new Map<string, Check>();
  const files = (taskId: string) => {
    let f = kept.get(taskId);
    if (!f) {
      f = {};
      kept.set(taskId, f);
    }
    return f;
  };
  const task = (id: string) => world.tasks.find((t) => t.id === id);
  const checkouts = (taskId: string) => task(taskId)?.checkouts ?? [];
  const requirementsOf = (taskId: string) => {
    const f = files(taskId);
    return f["bugfix.md"] ?? f["requirements.md"] ?? "";
  };

  const installing = params.get("install");
  const seeded = (taskId: string) =>
    Object.assign(
      files(taskId),
      worldOf(taskId) === KIRO
        ? { "requirements.md": API_REQUIREMENTS, "design.md": API_DESIGN, "tasks.md": TASKS }
        : worldOf(taskId).files,
    );
  const installState = (ok: boolean, running: boolean, error: string | null = null) => ({
    toolset: worldOf("t-login").toolset.id,
    version: worldOf("t-login").toolset.version,
    ok,
    error,
    running,
  });
  if (task("t-login")) {
    // An install that has not finished, or failed, has written nothing yet.
    if (installing === "failed") {
      installs.set("t-login", installState(false, false, "npx: command not found"));
    } else if (installing === "running") {
      installs.set("t-login", installState(false, true));
    } else {
      seeded("t-login");
    }
    // Check against spec is Kiro's extra.
    if (worldOf("t-login") === KIRO)
      checks.set("t-login:c-login-api", {
        sha: "a1b2c3d4",
        at: Date.now() - 600_000,
        stale: true,
        results: [
          { id: "R-1", verdict: "met", evidence: "src/auth/session.ts:41, session.test.ts" },
          { id: "R-2", verdict: "not met", evidence: "no retry in login.ts yet" },
          { id: "R-3", verdict: "unclear", evidence: "the 409 body is not sent yet" },
        ],
      });
  }
  if (task("t-audit")) files("t-audit")["requirements.md"] = AUDIT_DRAFT;

  const view = (taskId: string): SpecView => {
    const t = task(taskId);
    const f = files(taskId);
    const folders = checkouts(taskId).map((c) => c.project_name);
    const list = Object.keys(f)
      .sort()
      .map((path) => ({
        path,
        text: f[path],
        steps: worldOf(taskId).holdsSteps(path) ? steps(f[path], folders) : [],
      }));
    const repos: RepoSpecView[] = checkouts(taskId).map((c) => {
      const project = world.projects.find((p) => p.id === c.project_id);
      const own = list
        .flatMap((x) => x.steps)
        .filter((s) => s.repos.length === 0 || s.repos.includes(c.project_name));
      return {
        checkout_id: c.id,
        repo: c.project_name,
        folder: c.project_name,
        commit: (project?.spec_commit ?? "whole") as SpecCommit,
        done: own.filter((s) => s.done).length,
        total: own.length,
        changed_since_commit:
          list.length > 0 && project?.spec_commit !== "none" && !committed.has(`${taskId}:${c.id}`),
        check: checks.get(`${taskId}:${c.id}`) ?? null,
      };
    });
    return structuredClone({
      toolset: worldOf(taskId).toolset,
      dir: `~/.villain-worktrees/${t?.branch ?? taskId}/spec`,
      files: list,
      unreadable: [],
      repos,
      install: installs.get(taskId) ?? null,
    });
  };
  /** Repositories whose spec was committed since it last changed, as `task:checkout`. */
  const committed = new Set<string>();
  const written = (taskId: string) => {
    for (const key of committed) if (key.startsWith(`${taskId}:`)) committed.delete(key);
  };
  specDisk.write = (taskId, path, text) => {
    files(taskId)[path] = text;
    written(taskId);
    void emit("spec:changed", taskId);
  };
  specDisk.remove = (taskId, path) => {
    delete files(taskId)[path];
    written(taskId);
    void emit("spec:changed", taskId);
  };

  return {
    read_spec: (a) => view(a.taskId as string),
    ask_for_spec: () =>
      "Write or update the spec in /mock/spec before you change any code. Then stop: say the spec is ready, and change no code until the user tells you to go on.",
    write_spec_file: (a) => {
      const disk = files(a.taskId as string)[a.path as string];
      // The backend's refusal: the file is no longer what the editor began from.
      if (typeof a.base === "string" && !a.force && (disk ?? "") !== a.base) return false;
      files(a.taskId as string)[a.path as string] = a.text as string;
      written(a.taskId as string);
      return true;
    },
    remove_spec_file: (a) => {
      const disk = files(a.taskId as string)[a.path as string];
      if (typeof a.base === "string" && disk !== undefined && disk !== a.base) return false;
      delete files(a.taskId as string)[a.path as string];
      written(a.taskId as string);
      return true;
    },
    // Retry: the install runs a moment, then has written the toolset's files.
    install_spec_toolset: (a) => {
      const taskId = a.taskId as string;
      const running = installState(false, true);
      installs.set(taskId, running);
      setTimeout(() => {
        seeded(taskId);
        const ok = installState(true, false);
        installs.set(taskId, ok);
        void emit("spec:install", { taskId, state: ok });
      }, 1200);
      return running;
    },
    // Commit spec: each repository's own mode decides; one in
    // `__mock.spec.commitFails` fails, the others keep theirs (SPEC-35).
    commit_spec_cmd: (a) => {
      const taskId = a.taskId as string;
      return checkouts(taskId).map((c): RepoResult => {
        const mode = world.projects.find((p) => p.id === c.project_id)?.spec_commit ?? "whole";
        if (specDisk.commitFails.includes(c.project_name)) {
          return {
            checkout_id: c.id,
            repo: c.project_name,
            ok: false,
            detail: "the pre-commit hook refused it",
          };
        }
        if (mode !== "none") committed.add(`${taskId}:${c.id}`);
        return {
          checkout_id: c.id,
          repo: c.project_name,
          ok: true,
          detail: mode === "none" ? "kept by the app" : "committed 1a2b3c4",
        };
      });
    },
    draft_spec: async (a) => {
      const taskId = a.taskId as string;
      const f = files(taskId);
      const part = a.part as SpecPart;
      const path =
        part === "requirements"
          ? (["bugfix.md", "requirements.md"].find((p) => p in f) ??
            (a.kind === "bugfix" ? "bugfix.md" : "requirements.md"))
          : `${part}.md`;
      const folders = checkouts(taskId).map((c) => c.project_name);
      const text =
        part === "tasks"
          ? folders.map((folder) => `# ${folder}\n\n${DRAFT.tasks(folder).trim()}\n`).join("\n")
          : DRAFT[part](folders.join(", "));
      const before = f[path] ?? null;
      f[path] = await stream(a.requestId, path, text);
      return { path, before, after: f[path] };
    },
    apply_spec_answers: async (a) => {
      const taskId = a.taskId as string;
      const path = files(taskId)["bugfix.md"] ? "bugfix.md" : "requirements.md";
      const before = requirementsOf(taskId);
      if (!questions(before).some((q) => q.answer)) throw "no open question has an answer yet";
      const after = applied(before);
      files(taskId)[path] = await stream(a.requestId, path, after, 30);
      const old = requirements(before).map((r) => r.id);
      return {
        path,
        before,
        after: files(taskId)[path],
        changed: [],
        added: requirements(after)
          .map((r) => r.id)
          .filter((id) => !old.includes(id)),
        removed: [],
      };
    },
    check_spec: async (a) => {
      await new Promise((r) => setTimeout(r, 900));
      const taskId = a.taskId as string;
      const folder = checkouts(taskId).find((c) => c.id === a.checkoutId)?.project_name ?? "";
      const mine = requirements(requirementsOf(taskId)).filter(
        (r) => r.repos.length === 0 || r.repos.includes(folder),
      );
      const results: SpecVerdict[] = mine.map((r, i) => ({
        id: r.id,
        verdict: i === 0 ? "met" : "unclear",
        evidence: i === 0 ? "src/auth/session.ts:41" : "needs a run to tell",
      }));
      const check = { sha: "e5f6a7b8", at: Date.now(), results, stale: false };
      checks.set(`${taskId}:${a.checkoutId as string}`, check);
      return check;
    },
    set_project_specs: (a) => {
      const p = world.projects.find((x) => x.id === a.projectId);
      if (p) {
        p.spec_toolset = (a.toolset as string | null) ?? null;
        p.spec_commit = a.commit as SpecCommit;
        p.spec_folder = (a.folder as string) === "specs" ? null : (a.folder as string);
      }
      return null;
    },
  };
}
