import { useRef, useState, type DragEvent as ReactDragEvent } from "react";
import { api } from "./api";

export function appendAttachmentPaths(draft: string, paths: readonly string[]): string {
  if (paths.length === 0) return draft;
  return `${draft}${draft ? "\n" : ""}${paths.map((path) => JSON.stringify(path)).join("\n")}`;
}

export function preventFileNavigation(event: DragEvent): void {
  if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
}

export function useChatDrop(
  paneId: string,
  over: boolean,
  append: (paths: string[]) => void,
  fail: (error: unknown) => void,
  clear: () => void,
) {
  const [dragging, setDragging] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const drop = (e: ReactDragEvent) => {
    if (!e.dataTransfer.types.includes("Files")) return;
    e.preventDefault();
    setDragging(false);
    if (over) return;
    if (savingRef.current) {
      fail("Still saving images. Drop them again when it finishes.");
      return;
    }
    const dropped = e.dataTransfer.files;
    const count = dropped.length;
    const files = Array.from({ length: Math.min(count, 10) }, (_, i) => dropped[i]);
    if (!count) return;
    savingRef.current = true;
    setSaving(true);
    clear();
    void (async () => {
      const errors: string[] = [];
      const paths: string[] = [];
      if (count > 10) errors.push(`${count - 10} files were not saved. Drop at most 10 at a time.`);
      for (const file of files) {
        if (file.size > 10 * 1024 * 1024) {
          errors.push(`${file.name}: File exceeds 10 MiB.`);
          continue;
        }
        try {
          paths.push(await api.saveChatImage(paneId, file.name, file));
        } catch (error) {
          errors.push(`${file.name}: ${String(error)}`);
        }
      }
      append(paths);
      if (errors.length) fail(errors.join(" "));
      savingRef.current = false;
      setSaving(false);
    })();
  };
  return { dragging, setDragging, saving, savingRef, drop };
}
