import { expect, test } from "bun:test";
import { detailsAnswers } from "../src/__mock__/item-details";
import { SCENARIOS } from "../src/__mock__/world";
import type { DetailsEvent, ItemDetails } from "../src/lib/types";

test("mock saves canonical details, preserves omitted fields and emits only changed writes", () => {
  const world = structuredClone(SCENARIOS.busy());
  const events: DetailsEvent[] = [];
  const commands = detailsAnswers(world, (event) => events.push(event));
  const taskId = world.tasks[0].id;
  expect(commands.add_tag({ tag: " Review " })).toBe("Review");
  expect(
    commands.set_task_details({
      taskId,
      patch: { display_name: " Alias ", tags: ["review", "REVIEW", " "], color: "data-2" },
    }),
  ).toEqual({
    display_name: "Alias",
    tags: ["Review"],
    color: "data-2",
  });
  expect(commands.set_task_details({ taskId, patch: { display_name: null } })).toEqual({
    display_name: null,
    tags: ["Review"],
    color: "data-2",
  });
  const count = events.length;
  commands.set_task_details({ taskId, patch: {} });
  commands.add_tag({ tag: "review" });
  expect(events).toHaveLength(count);
  expect(commands.list_tags({})).toEqual(["Review"]);
});

test("mock rejects invalid and missing targets without leaking catalog entries", () => {
  const world = structuredClone(SCENARIOS.busy());
  const events: DetailsEvent[] = [];
  const commands = detailsAnswers(world, (event) => events.push(event));
  const taskId = world.tasks[0].id;
  for (const patch of [
    { display_name: "bad\nname", tags: ["New"] },
    { display_name: "x".repeat(121) },
    { tags: ["x".repeat(41)] },
    { tags: Array.from({ length: 13 }, (_, i) => `${i}`) },
    { color: "red" },
  ])
    expect(() => commands.set_task_details({ taskId, patch })).toThrow();
  expect(() => commands.set_task_details({ taskId: "gone", patch: { tags: ["New"] } })).toThrow();
  expect(() => commands.set_chat_details({ room: "/missing", patch: { tags: ["New"] } })).toThrow();
  expect(commands.list_tags({})).toEqual([]);
  expect(events).toEqual([]);
});

test("mock room details follow open and waiting chat identities, not automatic titles", () => {
  const world = structuredClone(SCENARIOS.busy());
  const commands = detailsAnswers(world, () => {});
  const waiting = world.waiting.find((p) => p.task_id === "chat")!;
  const details = commands.set_chat_details({
    room: waiting.cwd,
    patch: { display_name: "Saved" },
  }) as ItemDetails;
  const projected = commands.waiting_panes({}) as typeof world.waiting;
  expect(projected.find((p) => p.id === waiting.id)?.details).toEqual(details);
  expect(waiting.title).not.toBe("Saved");
  expect(() =>
    commands.set_chat_details({ room: "/Users/you/.villain-worktrees/_chat/henchman", patch: {} }),
  ).toThrow();
});

test("mock projects task and open room details without replacing source names", () => {
  const world = structuredClone(SCENARIOS.busy());
  const commands = detailsAnswers(world, () => {});
  const task = world.tasks[0];
  const pane = world.panes.find((p) => p.task_id === "chat" && p.kind === "agent")!;
  commands.set_task_details({ taskId: task.id, patch: { display_name: "Task alias" } });
  commands.set_chat_details({ room: pane.cwd, patch: { display_name: "Room alias" } });
  const tasks = commands.list_tasks({}) as typeof world.tasks;
  expect(tasks[0].details?.display_name).toBe("Task alias");
  expect(tasks[0].name).toBe(task.name);
  const replacement = { ...pane, id: "replacement", topic: "New CLI title" };
  world.panes = [replacement];
  const panes = commands.list_panes({}) as typeof world.panes;
  expect(panes[0].details?.display_name).toBe("Room alias");
  expect(panes[0].topic).toBe("New CLI title");
  commands.set_chat_details({ room: pane.cwd, patch: { display_name: null } });
  expect((commands.list_panes({}) as typeof world.panes)[0].details).toBeUndefined();
});
