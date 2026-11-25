import { useRef, useState, type DragEvent as ReactDragEvent } from "react";
import { api } from "./api";

export function appendAttachmentPaths(draft: string, paths: readonly string[]): string {
  if (paths.length === 0) return draft;
  return `${draft}${draft ? "\n" : ""}${paths.map((path) => JSON.stringify(path)).join("\n")}`;
}

/**
 * A path as a Mac terminal types a dropped file: every character a shell
 * would read specially escaped with a backslash. Claude Code turns a path
 * like that, pasted on its own, into an attached image.
 */
export function terminalPaths(paths: readonly string[]): string {
  return paths.map((path) => path.replace(/[^\w@%+=:,./-]/g, "\\$&")).join(" ");
}

/**
 * Save up to ten dropped images beside the pane's task (ACP-20, PANE-19).
 * Each that fails is named in `errors`; the rest are saved anyway.
 */
export async function saveDroppedImages(
  paneId: string,
  dropped: FileList,
): Promise<{ paths: string[]; errors: string[] }> {
  const count = dropped.length;
  const files = Array.from({ length: Math.min(count, 10) }, (_, i) => dropped[i]);
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
  return { paths, errors };
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
    if (!dropped.length) return;
    savingRef.current = true;
    setSaving(true);
    clear();
    void (async () => {
      const { paths, errors } = await saveDroppedImages(paneId, dropped);
      append(paths);
      if (errors.length) fail(errors.join(" "));
      savingRef.current = false;
      setSaving(false);
    })();
  };
  return { dragging, setDragging, saving, savingRef, drop };
}
