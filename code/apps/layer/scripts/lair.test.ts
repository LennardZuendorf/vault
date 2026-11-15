import { describe, expect, test } from "bun:test";
import { facts, isHenchman, lair, lairCounts, ran, shortCommand, sortTask } from "../src/lib/lair";
import type { CheckoutPr, PaneInfo, TaskView } from "../src/lib/types";

function pane(over: Partial<PaneInfo> = {}): PaneInfo {
  return {
    id: "p1",
    task_id: "t1",
    checkout_id: null,
    kind: "agent",
    title: "Claude Code",
    agent_id: "claude",
    cwd: "/w/t1",
    running: true,
    exit_code: null,
    started_at: "",
    last_output_at: "",
    notice: null,
    activity: "idle",
    activity_since: "",
    topic: null,
    acp: false,
    ...over,
  };
}

function pr(over: { merged?: boolean; closed?: boolean } = {}, row: Partial<CheckoutPr> = {}) {
  return {
    checkout_id: "c",
    repo: "acme/api",
    checks: [],
    reviews: [],
    past: [],
    verdict: "none",
    base: "main",
    changed: 1,
    to_reply: 0,
    error: null,
    pr: {
      number: 1,
      state: over.merged || over.closed ? "closed" : "open",
      merged: !!over.merged,
      draft: false,
    },
    ...row,
  } as unknown as CheckoutPr;
}

function task(id: string, changed = 0): TaskView {
  return {
    id,
    name: id,
    checkouts: [{ changed, exists: true, broken: false, status: null }],
    pane_count: 0,
  } as unknown as TaskView;
}

const why = (panes: PaneInfo[], rows: CheckoutPr[] = [], changed = 0) =>
  sortTask(facts(panes, rows, changed));

describe("the lair's groups (HEN-5)", () => {
  test("an agent that asks comes before everything a pull request says", () => {
    const red = pr({}, { verdict: "changes_requested" });
    expect(why([pane({ activity: "asking" })], [red])).toEqual({
      group: "needs_you",
      rank: 0,
      why: "its agent asks",
    });
  });

  test("an agent out of budget needs you, even while it shows a question", () => {
    expect(why([pane({ activity: "asking", notice: "usage_limit" })]).rank).toBe(1);
  });

  test("changes asked for need you only while no agent is working on them", () => {
    const red = pr({}, { verdict: "changes_requested" });
    expect(why([], [red]).why).toBe("changes asked for");
    expect(why([pane({ activity: "working" })], [red]).group).toBe("moving");
  });

  test("failed checks need you; a check still running does not", () => {
    const failed = pr(
      {},
      { checks: [{ name: "ci", status: "completed", conclusion: "failure", url: null }] },
    );
    const running = pr(
      {},
      { checks: [{ name: "ci", status: "in_progress", conclusion: null, url: null }] },
    );
    expect(why([], [failed]).why).toBe("checks failed");
    expect(why([], [running]).why).toBe("in review");
  });

  test("a task whose every pull request merged is to finish; a closed one does not count", () => {
    expect(why([], [pr({ merged: true }), pr({ closed: true })]).group).toBe("to_finish");
    expect(why([], [pr({ merged: true }), pr()]).group).toBe("moving");
  });

  test("a task with no agent, no PR and no change is not started; with changes it is quiet", () => {
    expect(why([]).group).toBe("not_started");
    expect(why([], [], 2)).toEqual({ group: "quiet", rank: 0, why: "nothing running" });
    expect(why([pane({ running: false })]).group).toBe("not_started");
  });

  test("a shell does not count as an agent", () => {
    expect(why([pane({ kind: "shell" })]).group).toBe("not_started");
  });
});

describe("the lair", () => {
  test("Needs you is ranked, and a chat that asks counts in it", () => {
    const panes = [
      pane({ task_id: "a", activity: "done" }),
      pane({ id: "p2", task_id: "b", activity: "asking" }),
      pane({ id: "c1", task_id: "chat", cwd: "/w/_chat/x", activity: "asking" }),
    ];
    const l = lair([task("a"), task("b"), task("c", 3)], panes, {});
    expect(l.groups.needs_you.map((r) => r.task.id)).toEqual(["b", "a"]);
    expect(l.groups.quiet.map((r) => r.task.id)).toEqual(["c"]);
    expect(lairCounts(l)).toEqual({
      open: 3,
      needsYou: 3,
      line: "3 open · 3 need you · 0 to finish · 0 moving · 0 not started · 1 quiet",
    });
  });

  test("the Henchman is not one of its own chats", () => {
    const henchman = pane({
      id: "h",
      task_id: "chat",
      cwd: "/w/_chat/henchman",
      activity: "asking",
    });
    expect(isHenchman(henchman)).toBe(true);
    expect(isHenchman(pane({ task_id: "chat", cwd: "/w/_chat/henchmen" }))).toBe(false);
    expect(lair([], [henchman], {}).chats).toEqual([]);
  });
});

describe("the Running rows (HEN-6)", () => {
  test("how long a process has run reads at a glance", () => {
    expect(ran(12)).toBe("12s");
    expect(ran(250)).toBe("4m");
    expect(ran(7200)).toBe("2h");
    expect(ran(7500)).toBe("2h 5m");
    expect(ran(3 * 86400 + 5)).toBe("3d");
  });

  test("a path in a command is cut to its last part, and words stay as they are", () => {
    expect(shortCommand("node /w/ACME-123/web/node_modules/.bin/vite --port 5173")).toBe(
      "node …/vite --port 5173",
    );
    expect(shortCommand("cargo watch -x test")).toBe("cargo watch -x test");
    expect(shortCommand("python3 -m http.server 8000")).toBe("python3 -m http.server 8000");
  });
});
