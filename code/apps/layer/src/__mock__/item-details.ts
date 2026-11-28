import type { DetailsEvent, DetailsPatch, ItemDetails } from "../lib/types";
import type { World } from "./world";

const blank = (): ItemDetails => ({ display_name: null, tags: [], color: null });
function text(raw: unknown, limit: number): string | null {
  if (typeof raw !== "string") throw new Error("Text must be a string.");
  if (/[\p{Cc}\u2028\u2029]/u.test(raw))
    throw new Error("Use single-line text without control characters.");
  const value = raw.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  if ([...value].length > limit) throw new Error(`Text must be at most ${limit} characters.`);
  return value || null;
}

export function detailsAnswers(world: World, notify: (event: DetailsEvent) => void) {
  const canonical = (tag: string, catalog: string[]) => {
    const existing = catalog.find((t) => t.toLowerCase() === tag.toLowerCase());
    if (existing) return existing;
    catalog.push(tag);
    return tag;
  };
  const edit = (kind: "task" | "chat", key: unknown, raw: unknown) => {
    const exists =
      kind === "task"
        ? world.tasks.some((t) => t.id === key)
        : [...world.panes, ...world.waiting].some(
            (p) =>
              p.cwd === key &&
              p.task_id === "chat" &&
              p.kind === "agent" &&
              !("checkout_id" in p && p.checkout_id) &&
              !/\/_chat\/(?:henchman|routines)(?:\/|$)/.test(String(key)),
          );
    if (!exists || typeof key !== "string") throw new Error("That item no longer exists.");
    if (!raw || typeof raw !== "object" || Array.isArray(raw))
      throw new Error("Invalid details patch.");
    const patch = raw as DetailsPatch;
    const map = kind === "task" ? world.taskDetails : world.chatDetails;
    const before = map[key] ?? blank();
    const details = structuredClone(before);
    const catalog = [...world.tagCatalog];
    if (patch.display_name !== undefined)
      details.display_name = patch.display_name === null ? null : text(patch.display_name, 120);
    if (patch.color !== undefined) {
      if (patch.color !== null && !/^data-[1-6]$/.test(patch.color))
        throw new Error("Choose a palette color.");
      details.color = patch.color;
    }
    if (patch.tags != null) {
      if (!Array.isArray(patch.tags)) throw new Error("Tags must be a list.");
      details.tags = [];
      for (const raw of patch.tags) {
        const tag = text(raw, 40);
        if (tag && !details.tags.some((t) => t.toLowerCase() === tag.toLowerCase()))
          details.tags.push(canonical(tag, catalog));
      }
      if (details.tags.length > 12) throw new Error("Assign at most 12 distinct tags.");
    }
    const catalog_changed = catalog.length !== world.tagCatalog.length;
    const changed = JSON.stringify(before) !== JSON.stringify(details) || catalog_changed;
    if (changed) {
      if (details.display_name || details.tags.length || details.color) map[key] = details;
      else delete map[key];
      world.tagCatalog = catalog;
      notify({ kind, catalog_changed });
    }
    return structuredClone(details);
  };
  return {
    add_tag: (a: Record<string, unknown>): unknown => {
      const tag = text(a.tag, 40);
      if (!tag) throw new Error("Enter a tag.");
      const before = world.tagCatalog.length;
      const saved = canonical(tag, world.tagCatalog);
      if (before !== world.tagCatalog.length) notify({ kind: "catalog", catalog_changed: true });
      return saved;
    },
    set_task_details: (a: Record<string, unknown>): unknown => edit("task", a.taskId, a.patch),
    set_chat_details: (a: Record<string, unknown>): unknown => edit("chat", a.room, a.patch),
    list_tags: (_a: Record<string, unknown>): unknown => [...world.tagCatalog],
    // A copy, as IPC would hand over: the same objects edited in place look
    // unchanged to the store (`lib/same.ts`), and the UI never sees the edit.
    list_tasks: (_a: Record<string, unknown>): unknown =>
      structuredClone(
        world.tasks.map((t) => ({
          ...t,
          ...(world.taskDetails[t.id] ? { details: world.taskDetails[t.id] } : {}),
        })),
      ),
    // A copy too: a pane pushed onto the same array read as no change.
    list_panes: (a: Record<string, unknown>): unknown =>
      structuredClone(
        world.panes
          .filter((p) => !a.taskId || p.task_id === a.taskId)
          .map((p) => ({
            ...p,
            ...(world.chatDetails[p.cwd] ? { details: world.chatDetails[p.cwd] } : {}),
          })),
      ),
    waiting_panes: (_a: Record<string, unknown>): unknown =>
      structuredClone(
        world.waiting.map((p) => ({
          ...p,
          ...(p.cwd && world.chatDetails[p.cwd] ? { details: world.chatDetails[p.cwd] } : {}),
        })),
      ),
  };
}
