import { describe, expect, test } from "bun:test";
import { mergeDisk } from "../src/lib/spec-edit";

describe("a spec file changing on disk under its editor (SPEC-28)", () => {
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
