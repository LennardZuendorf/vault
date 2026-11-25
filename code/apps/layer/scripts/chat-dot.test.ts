import { describe, expect, test } from "bun:test";
import { CHAT_TASK_ID } from "../src/lib/derive";
import { chatAtWork } from "../src/lib/lair";
import type { PaneInfo } from "../src/lib/types";

function chat(over: Partial<PaneInfo> = {}): PaneInfo {
  return {
    id: Math.random().toString(36),
    task_id: CHAT_TASK_ID,
    checkout_id: null,
    kind: "agent",
    title: "Claude Code",
    agent_id: "claude",
    cwd: "/Users/me/Library/Application Support/layer/_chat/abc",
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

describe("the top bar's Chat dot", () => {
  test("stays off while every running chat is idle, finished or asking", () => {
    const panes = [
      chat(),
      chat({ activity: "done" }),
      chat({ activity: "asking" }),
      chat({ running: false, exit_code: 0 }),
    ];
    expect(chatAtWork(panes)).toBe(false);
  });

  test("lights while a chat is at work, in a terminal or over ACP", () => {
    expect(chatAtWork([chat(), chat({ activity: "working" })])).toBe(true);
    expect(chatAtWork([chat({ activity: "working", acp: true })])).toBe(true);
  });

  test("leaves the Henchman, routine runs and task agents to their own tabs", () => {
    const working = { activity: "working" } as const;
    expect(
      chatAtWork([
        chat({ ...working, cwd: "/x/_chat/henchman" }),
        chat({ ...working, cwd: "/x/_chat/routines/daily/run-1" }),
        chat({ ...working, task_id: "t1" }),
      ]),
    ).toBe(false);
  });

  test("does not count an exited chat whose last word was working", () => {
    expect(chatAtWork([chat({ activity: "working", running: false })])).toBe(false);
  });
});
