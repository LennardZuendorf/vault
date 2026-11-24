import { describe, expect, test } from "bun:test";
import { isSaveShortcut, mergeDisk, shortcutSaves } from "../src/lib/spec-edit";
import { reconcile } from "../src/lib/specs";
import type { SpecView } from "../src/lib/types";

describe("a spec file changing on disk under its editor (SPEC-16)", () => {
  test("a clean editor takes the file from disk", () => {
    const merged = mergeDisk(
      { text: "- [ ] 1. A", dirty: false, base: "- [ ] 1. A" },
      "- [x] 1. A",
    );
    expect(merged).toEqual({ text: "- [x] 1. A", conflict: false });
  });

  test("a dirty editor keeps its text and reports a conflict when the file changed", () => {
    const merged = mergeDisk(
      { text: "- [ ] 1. A, mine", dirty: true, base: "- [ ] 1. A" },
      "- [x] 1. A",
    );
    expect(merged).toEqual({ text: "- [ ] 1. A, mine", conflict: true });
  });

  test("a dirty editor with an unchanged file reports no conflict", () => {
    const merged = mergeDisk(
      { text: "- [ ] 1. A, mine", dirty: true, base: "- [ ] 1. A" },
      "- [ ] 1. A",
    );
    expect(merged).toEqual({ text: "- [ ] 1. A, mine", conflict: false });
  });
});

const key = (k: string, meta = true) => ({ metaKey: meta, ctrlKey: false, key: k });

describe("the save shortcut", () => {
  test("saves a file nobody else changed", () => {
    expect(shortcutSaves(key("s"), false)).toBe(true);
    expect(shortcutSaves(key("Enter"), false)).toBe(true);
  });

  test("is refused while the file changed on disk, so only Save mine overwrites it", () => {
    expect(isSaveShortcut(key("s"))).toBe(true);
    expect(shortcutSaves(key("s"), true)).toBe(false);
    expect(shortcutSaves(key("Enter"), true)).toBe(false);
  });

  test("is no shortcut without the modifier or for another key", () => {
    expect(isSaveShortcut(key("s", false))).toBe(false);
    expect(isSaveShortcut(key("a"))).toBe(false);
  });
});

const file = (path: string, text: string) => ({ path, text, steps: [] });
const specOf = (files: ReturnType<typeof file>[]) => ({ files }) as unknown as SpecView;

describe("the editors once the files are read again (SPEC-16)", () => {
  const typed = { text: "mine", base: "was", conflict: false };

  test("an agent writing the file under a dirty editor is a conflict, and the text is kept", () => {
    const next = reconcile({ "tasks.md": typed }, specOf([file("tasks.md", "theirs")]));
    expect(next["tasks.md"]).toEqual({ text: "mine", base: "was", conflict: true });
  });

  test("a file still as the editing began is no conflict, and the editors are left as they were", () => {
    const edits = { "tasks.md": typed };
    expect(reconcile(edits, specOf([file("tasks.md", "was")]))).toBe(edits);
  });

  test("typing back to what the file now holds leaves nothing to keep", () => {
    const next = reconcile({ "tasks.md": typed }, specOf([file("tasks.md", "mine")]));
    expect(next).toEqual({});
  });

  test("a file removed under a dirty editor is a conflict", () => {
    const next = reconcile({ "tasks.md": typed }, specOf([]));
    expect(next["tasks.md"].conflict).toBe(true);
    expect(next["tasks.md"].text).toBe("mine");
  });
});
