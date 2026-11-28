import { expect, test } from "bun:test";
import {
  detailsDraft,
  detailsPatch,
  normalizeTags,
  tagError,
} from "../src/lib/item-details-editor";

test("an untouched draft sends no fields and never mutates saved details", () => {
  const saved = { display_name: "Alias", tags: ["Focus"], color: "data-1" as const };
  const draft = detailsDraft(saved);
  expect(draft).toEqual({ display_name: "Alias", tags: ["Focus"], color: "data-1" });
  expect(detailsPatch(saved, draft, ["Focus"])).toEqual({});
  draft.tags.push("Later");
  expect(saved.tags).toEqual(["Focus"]);
});

test("edited fields use explicit clears and leave other fields absent", () => {
  const saved = { display_name: "Alias", tags: ["Focus"], color: "data-1" as const };
  expect(detailsPatch(saved, { ...detailsDraft(saved), display_name: "  " }, [])).toEqual({
    display_name: null,
  });
  expect(detailsPatch(saved, { display_name: "Alias", tags: [], color: null }, [])).toEqual({
    tags: [],
    color: null,
  });
});

test("tags reuse catalog spelling, preserve order, ignore blanks and case duplicates", () => {
  expect(normalizeTags([" new ", "", "focus", "NEW", "Other"], ["Focus"])).toEqual([
    "new",
    "Focus",
    "Other",
  ]);
  expect(
    detailsPatch(undefined, { display_name: " Name ", tags: ["focus"], color: null }, ["Focus"]),
  ).toEqual({ display_name: "Name", tags: ["Focus"] });
});

test("validation rejects raw controls before trimming and counts Unicode scalars", () => {
  for (const text of ["bad\n", "\tbad", "bad\u0000", "bad\u2028", "bad\u2029"]) {
    expect(tagError(text)).toBeTruthy();
    expect(() =>
      detailsPatch(undefined, { display_name: text, tags: [], color: null }, []),
    ).toThrow();
  }
  expect(tagError("😀".repeat(40))).toBeNull();
  expect(tagError("😀".repeat(41))).toBeTruthy();
  expect(tagError(" ")).toBeTruthy();
  expect(() =>
    detailsPatch(
      undefined,
      {
        display_name: "😀".repeat(121),
        tags: [],
        color: null,
      },
      [],
    ),
  ).toThrow();
});

test("validation rejects too many distinct tags and non-palette colors", () => {
  expect(() =>
    normalizeTags(
      Array.from({ length: 13 }, (_, i) => `Tag ${i}`),
      [],
    ),
  ).toThrow();
  expect(() =>
    detailsPatch(
      undefined,
      {
        display_name: "",
        tags: [],
        color: "red",
      },
      [],
    ),
  ).toThrow();
});
