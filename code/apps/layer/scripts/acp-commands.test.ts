import { describe, expect, test } from "bun:test";
import { suggestedCommands } from "../src/lib/acpCommands";

const cmd = (name: string, description = name) => ({ name, description, hint: null });

describe("the commands offered while typing a slash (ACP-22)", () => {
  test("Layer's /clear and /new come after the agent's own", () => {
    expect(suggestedCommands([cmd("review")], "/").map((c) => c.name)).toEqual([
      "review",
      "clear",
      "new",
    ]);
  });

  test("an agent's command by the same name is offered once, as the agent's", () => {
    const offered = suggestedCommands([cmd("clear", "theirs")], "/cl");
    expect(offered.map((c) => c.description)).toEqual(["theirs"]);
  });

  test("nothing is offered once the command has words after it, or for plain text", () => {
    expect(suggestedCommands([cmd("review")], "/review this")).toEqual([]);
    expect(suggestedCommands([cmd("review")], "hello")).toEqual([]);
  });
});
