import type { DetailsPatch, ItemDetails } from "./types";

export interface DetailsDraft {
  display_name: string;
  tags: string[];
  color: string | null;
}
export const ITEM_COLORS: { value: ItemDetails["color"]; label: string }[] = [
  { value: null, label: "No color" },
  { value: "data-1", label: "Lavender" },
  { value: "data-2", label: "Cyan" },
  { value: "data-3", label: "Rose" },
  { value: "data-4", label: "Blue" },
  { value: "data-5", label: "Amber" },
  { value: "data-6", label: "Mint" },
];

function singleLineError(raw: string): string | null {
  return /[\p{Cc}\u2028\u2029]/u.test(raw)
    ? "Use single-line text without control characters."
    : null;
}

/** Inspect transfer text before a single-line input can silently remove CR/LF. */
export function rejectInvalidTextTransfer(
  raw: string,
  event: { preventDefault: () => void },
  setError: (error: string) => void,
) {
  const error = singleLineError(raw);
  if (!error) return;
  event.preventDefault();
  setError(error);
}

function text(raw: string, limit: number): string {
  const error = singleLineError(raw);
  if (error) throw new Error(error);
  const value = raw.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  if ([...value].length > limit) throw new Error(`Use at most ${limit} characters.`);
  return value;
}

export function tagError(tag: string): string | null {
  try {
    if (!text(tag, 40)) return "Enter a tag.";
    return null;
  } catch (error) {
    return (error as Error).message;
  }
}

export function normalizeTags(tags: string[], catalog: string[]): string[] {
  const result: string[] = [];
  for (const raw of tags) {
    const tag = text(raw, 40);
    if (!tag || result.some((t) => t.toLowerCase() === tag.toLowerCase())) continue;
    result.push(catalog.find((t) => t.toLowerCase() === tag.toLowerCase()) ?? tag);
  }
  if (result.length > 12) throw new Error("Assign at most 12 distinct tags.");
  return result;
}

export function detailsDraft(details?: ItemDetails): DetailsDraft {
  return {
    display_name: details?.display_name ?? "",
    tags: [...(details?.tags ?? [])],
    color: details?.color ?? null,
  };
}

export function detailsPatch(
  details: ItemDetails | undefined,
  draft: DetailsDraft,
  catalog: string[],
): DetailsPatch {
  const name = text(draft.display_name, 120) || null;
  const tags = normalizeTags(draft.tags, catalog);
  const color = ITEM_COLORS.find((c) => c.value === draft.color);
  if (!color) throw new Error("Choose a palette color.");
  const patch: DetailsPatch = {};
  if (name !== (details?.display_name ?? null)) patch.display_name = name;
  if (JSON.stringify(tags) !== JSON.stringify(details?.tags ?? [])) patch.tags = tags;
  if (color.value !== (details?.color ?? null)) patch.color = color.value;
  return patch;
}
