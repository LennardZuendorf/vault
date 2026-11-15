import { describe, expect, test } from "bun:test";
import { keyProjects, splitKeys } from "../src/lib/issue-keys";

describe("ticket keys in what an agent says (ACP-18)", () => {
  test("the projects come from known keys, each once; a GitHub issue names none", () => {
    expect(keyProjects(["ACME-12", "OPS-3", "ACME-40", null, "acme/web#12", undefined])).toBe(
      "ACME|OPS",
    );
  });

  test("a key of a known project is set apart wherever it stands in the sentence", () => {
    expect(splitKeys("(ACME-123, ACME-4) are open; OPS-9 too", "ACME")).toEqual([
      "(",
      { key: "ACME-123" },
      ", ",
      { key: "ACME-4" },
      ") are open; OPS-9 too",
    ]);
  });

  test("words that only look like keys stay words", () => {
    const text = "SHA-256 and UTF-8, test-e2e, xACME-1, ACME-12b";
    expect(splitKeys(text, "ACME")).toEqual([text]);
  });

  test("with no project known, nothing is a key", () => {
    expect(splitKeys("ACME-1", "")).toEqual(["ACME-1"]);
  });
});
