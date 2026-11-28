import { describe, expect, test } from "bun:test";
import { versionLine } from "../src/lib/version";

describe("the version Settings ends with (SET-11)", () => {
  test("a release says its number", () => {
    expect(versionLine("0.17.0", undefined).text).toBe("Layer 0.17.0");
    expect(versionLine("0.17.0", "").text).toBe("Layer 0.17.0");
  });

  test("a build made on this Mac says its commit, and which release it follows", () => {
    const line = versionLine("0.17.0", "c3e5bbe-dirty");
    expect(line.text).toBe("Layer c3e5bbe-dirty");
    expect(line.title).toContain("after release 0.17.0");
  });
});
