import { expect, test } from "bun:test";
import { appendAttachmentPaths } from "../src/lib/chatDrop";

test("adds quoted paths without replacing the draft", () => {
  expect(appendAttachmentPaths("", ["/task/attachments/Screen Shot.png"])).toBe(
    '"/task/attachments/Screen Shot.png"',
  );
  expect(appendAttachmentPaths("  Exact text\n", ["/a", '/b"c'])).toBe(
    '  Exact text\n\n"/a"\n"/b\\"c"',
  );
  expect(appendAttachmentPaths("unchanged", [])).toBe("unchanged");
});
