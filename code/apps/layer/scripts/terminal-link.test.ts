import { describe, expect, test } from "bun:test";
import { openTerminalLink } from "../src/lib/terminalLink";

function click(uri: string, open: (url: string) => Promise<void> = async () => {}) {
  const opened: string[] = [];
  const failed: string[] = [];
  openTerminalLink(
    uri,
    (url) => {
      opened.push(url);
      return open(url);
    },
    (text) => failed.push(text),
  );
  return { opened, failed };
}

describe("a link clicked in a terminal (PANE-23)", () => {
  test("a web link opens in the browser without asking", () => {
    expect(click("https://github.com/acme/app/pull/12")).toEqual({
      opened: ["https://github.com/acme/app/pull/12"],
      failed: [],
    });
  });

  test("a file link is not handed to the Mac, and says why", () => {
    const { opened, failed } = click("file:///Users/me/run.command");
    expect(opened).toEqual([]);
    expect(failed[0]).toContain("Only web and mail links");
  });

  test("a browser that will not open is said, not swallowed", async () => {
    const { failed } = click("https://example.com", () => Promise.reject(new Error("no")));
    await Promise.resolve();
    await Promise.resolve();
    expect(failed).toEqual(["Couldn't open the browser. Try again."]);
  });
});
