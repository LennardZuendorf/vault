import { describe, expect, test } from "bun:test";
import { layerSaid, mergeEntries } from "../src/lib/acpEntries";
import type { AcpEntry } from "../src/lib/types";

const say = (index: number, text: string, rev = index): AcpEntry => ({
  index,
  rev,
  kind: "agent",
  text,
});
const view = (entries: AcpEntry[], base = 0, reset = false) => ({ entries, base, reset });

describe("a conversation's entries as they change (ACP-7)", () => {
  test("a token streamed into the last reply replaces only that entry", () => {
    const held = [say(0, "hi"), say(1, "Look")];
    const next = mergeEntries(held, view([say(1, "Looking", 5)]));
    expect(next.map((e) => e.index)).toEqual([0, 1]);
    expect(next[0]).toBe(held[0]);
    expect(next[1]).toEqual(say(1, "Looking", 5));
    expect(held[1]).toEqual(say(1, "Look"));
  });

  test("new entries go after, in order, whatever order they came in", () => {
    const next = mergeEntries([say(0, "a")], view([say(2, "c"), say(1, "b")]));
    expect(next.map((e) => e.index)).toEqual([0, 1, 2]);
  });

  test("an entry changed in the middle stays where it was", () => {
    const held = [say(0, "a"), say(1, "b"), say(2, "c")];
    expect(mergeEntries(held, view([say(1, "B", 9)])).map((e) => e.index)).toEqual([0, 1, 2]);
  });

  test("entries below the base are dropped, held or new", () => {
    const held = [say(0, "a"), say(1, "b"), say(2, "c")];
    expect(mergeEntries(held, view([say(1, "B", 9), say(3, "d")], 2)).map((e) => e.index)).toEqual([
      2, 3,
    ]);
  });

  test("a reset replaces everything held", () => {
    const held = [say(0, "a"), say(1, "b")];
    expect(mergeEntries(held, view([say(5, "x")], 5, true)).map((e) => e.index)).toEqual([5]);
  });

  test("nothing changed is the list already held", () => {
    const held = [say(0, "a")];
    expect(mergeEntries(held, view([]))).toBe(held);
  });
});

describe("layerSaid", () => {
  test("a note from Layer folds to its first line, with its tag apart", () => {
    const said = layerSaid(
      "[villain-layer · finished] These finished a turn and wait.\n\n- ACME-1 · Claude Code",
    );
    expect(said).toEqual({
      tag: "finished",
      first: "These finished a turn and wait.",
      rest: "- ACME-1 · Claude Code",
    });
  });

  test("a note with no tag and one line has nothing to unfold", () => {
    expect(layerSaid("[villain-layer] api has just been added")).toEqual({
      tag: "",
      first: "api has just been added",
      rest: "",
    });
  });

  test("the user's own words, even quoting Layer, stay as they are", () => {
    expect(layerSaid("why did [villain-layer · finished] say that?")).toBeNull();
  });
});
