import { describe, expect, test } from "bun:test";
import { suggestToolset } from "../src/lib/spec-toolset";

const known = ["kiro", "openspec", "superpowers"];

describe("the toolset Start work offers (§19)", () => {
  test("it is the one every picked repository prefers", () => {
    const picked = [{ spec_toolset: "openspec" }, { spec_toolset: "openspec" }];
    expect(suggestToolset(picked, known, "superpowers")).toBe("openspec");
  });

  test("repositories that disagree, or that prefer none, leave it to the app's default", () => {
    expect(suggestToolset([{ spec_toolset: "openspec" }, {}], known, "superpowers")).toBe(
      "superpowers",
    );
    expect(
      suggestToolset([{ spec_toolset: "openspec" }, { spec_toolset: "kiro" }], known, null),
    ).toBe("kiro");
    expect(suggestToolset([], known, null)).toBe("kiro");
  });

  test("a toolset this Mac does not have is passed over", () => {
    const picked = [{ spec_toolset: "teammates" }];
    expect(suggestToolset(picked, known, "openspec")).toBe("openspec");
    expect(suggestToolset(picked, known, "also-gone")).toBe("kiro");
  });
});
