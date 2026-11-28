import { afterEach, describe, expect, test } from "bun:test";
import { api } from "../src/lib/api";
import { useSpecs } from "../src/lib/specs";
import type { SpecView } from "../src/lib/types";

const spec = (text: string) =>
  ({ files: [{ path: "design.md", text, steps: [] }], repos: [] }) as unknown as SpecView;

const real = { write: api.writeSpecFile, read: api.readSpec, remove: api.removeSpecFile };
afterEach(() => {
  api.writeSpecFile = real.write;
  api.readSpec = real.read;
  api.removeSpecFile = real.remove;
});

const opened = (edits: Record<string, { text: string; base: string; conflict: boolean }>) =>
  useSpecs.setState({
    byTask: {
      t1: {
        loaded: true,
        spec: spec("what I began from"),
        edits,
        drafting: null,
        checking: [],
        last: null,
        committed: null,
      },
    },
  });

describe("saving a spec file over what an agent wrote (SPEC-37)", () => {
  test("a refused save keeps the editor's text and marks it a conflict, and Save mine forces it", async () => {
    const sent: unknown[][] = [];
    let disk = "agent's";
    api.readSpec = async () => spec(disk);
    api.writeSpecFile = async (_task, _path, text, base, force) => {
      sent.push([text, base, force]);
      if (!force && base !== disk) return false;
      disk = text;
      return true;
    };
    useSpecs.setState({
      byTask: {
        t1: {
          loaded: true,
          spec: spec("what I began from"),
          edits: { "design.md": { text: "mine", base: "what I began from", conflict: false } },
          drafting: null,
          checking: [],
          last: null,
          committed: null,
        },
      },
    });

    const first = await useSpecs.getState().save("t1", "design.md");

    expect(first).toBe(false);
    expect(disk).toBe("agent's");
    expect(useSpecs.getState().byTask.t1.edits["design.md"]).toEqual({
      text: "mine",
      base: "what I began from",
      conflict: true,
    });

    const forced = await useSpecs.getState().save("t1", "design.md", true);

    expect(forced).toBe(true);
    expect(disk).toBe("mine");
    expect(sent).toEqual([
      ["mine", "what I began from", false],
      ["mine", "what I began from", true],
    ]);
    expect(useSpecs.getState().byTask.t1.edits["design.md"]).toBeUndefined();
  });

  test("what is typed while a save is under way is kept, and the file's next read is no conflict", async () => {
    let disk = "what I began from";
    api.readSpec = async () => spec(disk);
    opened({ "design.md": { text: "saved", base: "what I began from", conflict: false } });
    api.writeSpecFile = async (_task, _path, text) => {
      // The user types on while the write is out.
      useSpecs.getState().edit("t1", "design.md", "saved, and more");
      disk = text;
      return true;
    };

    expect(await useSpecs.getState().save("t1", "design.md")).toBe(true);

    expect(disk).toBe("saved");
    expect(useSpecs.getState().byTask.t1.edits["design.md"]).toEqual({
      text: "saved, and more",
      base: "saved",
      conflict: false,
    });
  });
});

describe("Undo of a draft (SPEC-20)", () => {
  test("puts the old text back only while the file still holds what the draft wrote", async () => {
    let disk = "the agent's, since the draft";
    api.readSpec = async () => spec(disk);
    const sent: unknown[][] = [];
    api.writeSpecFile = async (_task, _path, text, base) => {
      sent.push([text, base]);
      if (base !== undefined && base !== disk) return false;
      disk = text;
      return true;
    };
    opened({});
    useSpecs.setState((s) => ({
      byTask: {
        t1: {
          ...s.byTask.t1,
          last: { path: "design.md", before: "old", after: "drafted", results: null },
        },
      },
    }));

    await expect(useSpecs.getState().undo("t1")).rejects.toThrow("changed since");

    expect(disk).toBe("the agent's, since the draft");
    expect(sent).toEqual([["old", "drafted"]]);
    expect(useSpecs.getState().byTask.t1.last).toBeNull();
  });

  test("removes a file the draft made, but not one an agent has written since", async () => {
    const removed: unknown[][] = [];
    api.readSpec = async () => spec("");
    api.removeSpecFile = async (_task, path, base) => {
      removed.push([path, base]);
      return false;
    };
    opened({});
    useSpecs.setState((s) => ({
      byTask: {
        t1: {
          ...s.byTask.t1,
          last: { path: "design.md", before: null, after: "drafted", results: null },
        },
      },
    }));

    await expect(useSpecs.getState().undo("t1")).rejects.toThrow("changed since");

    expect(removed).toEqual([["design.md", "drafted"]]);
  });
});
