import { expect, test } from "bun:test";
import { appendAttachmentPaths, terminalPaths } from "../src/lib/chatDrop";

test("adds quoted paths without replacing the draft", () => {
  expect(appendAttachmentPaths("", ["/task/attachments/Screen Shot.png"])).toBe(
    '"/task/attachments/Screen Shot.png"',
  );
  expect(appendAttachmentPaths("  Exact text\n", ["/a", '/b"c'])).toBe(
    '  Exact text\n\n"/a"\n"/b\\"c"',
  );
  expect(appendAttachmentPaths("unchanged", [])).toBe("unchanged");
});

test("escapes paths for a terminal as a Mac terminal does", () => {
  expect(terminalPaths(["/task/attachments/Screen Shot (2).png", "/a/b-c_d.png"])).toBe(
    "/task/attachments/Screen\\ Shot\\ \\(2\\).png /a/b-c_d.png",
  );
  expect(terminalPaths(["/it's.png"])).toBe("/it\\'s.png");
});
