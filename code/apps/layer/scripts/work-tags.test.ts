import { expect, test } from "bun:test";
import * as grouping from "../src/lib/grouping";

test("tag groups sort named headings, retain input stage order and have a distinct final untagged ID", () => {
  const items = [
    { id: "working", details: { tags: ["Zulu", "Alpha"] } },
    { id: "reviewing", details: { tags: ["Alpha"] } },
    { id: "done", details: { tags: ["Alpha", "No tag"] } },
    { id: "untagged" },
  ];
  expect(typeof grouping.byTag).toBe("function");
  const groups = grouping.byTag(items);
  expect(groups.map((g) => [g.label, g.items.map((t) => t.id)])).toEqual([
    ["Alpha", ["working", "reviewing", "done"]],
    ["No tag", ["done"]],
    ["Zulu", ["working"]],
    ["No tag", ["untagged"]],
  ]);
  expect(new Set(groups.map((g) => g.id)).size).toBe(4);
});

test("tag mode flattens only filtered stored sections, once per task ID", () => {
  const task = { id: "plain" },
    epic = { id: "epic" },
    sub = { id: "sub" },
    jira = { id: "jira" };
  expect(typeof grouping.storedWorkItems).toBe("function");
  expect(
    grouping
      .storedWorkItems({
        tasks: [task],
        layerEpics: [epic],
        subsOf: { epic: [sub], excludedEpic: [{ id: "hidden" }] },
        jiraEpics: [{ tasks: [jira, sub] }],
      })
      .map((t) => t.id),
  ).toEqual(["plain", "epic", "sub", "jira"]);
});

test("tag memberships deduplicate case variants without moving stages or changing catalog spelling", () => {
  const groups = grouping.byTag([
    { id: "working", details: { tags: ["release", "RELEASE"] } },
    { id: "done", details: { tags: ["release"] } },
  ]);
  expect(groups.map((g) => [g.label, g.items.map((t) => t.id)])).toEqual([
    ["release", ["working", "done"]],
  ]);
  expect(grouping.byTag([])).toEqual([]);
});
