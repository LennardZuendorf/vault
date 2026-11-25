import { afterEach, describe, expect, test } from "bun:test";
import { api } from "../src/lib/api";
import { useSpecs } from "../src/lib/specs";
import type { SpecView } from "../src/lib/types";

const spec = (text: string) =>
  ({ files: [{ path: "design.md", text, steps: [] }], repos: [] }) as unknown as SpecView;

const real = { write: api.writeSpecFile, read: api.readSpec };
afterEach(() => {
  api.writeSpecFile = real.write;
  api.readSpec = real.read;
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
});
