/** A spec file's editor, as far as a change on disk needs to know it. */
export interface Editor {
  text: string;
  /** The editor holds text that is not in the file. */
  dirty: boolean;
  /** What the file held when the editing began. */
  base: string;
}

/** Cmd+S or Cmd+Enter (Ctrl on other keyboards): what saves what is being written. */
export function isSaveShortcut(e: { metaKey: boolean; ctrlKey: boolean; key: string }): boolean {
  return (e.metaKey || e.ctrlKey) && (e.key === "Enter" || e.key === "s");
}

/**
 * Whether that shortcut writes the file. Not while the file changed under the
 * editor: it would overwrite what an agent wrote with no word, so over theirs
 * is only the button, "Save mine".
 */
export function shortcutSaves(
  e: { metaKey: boolean; ctrlKey: boolean; key: string },
  conflict: boolean,
): boolean {
  return isSaveShortcut(e) && !conflict;
}

/**
 * What an editor shows once the file on disk is read again (SPEC-16). A clean
 * editor takes the file. A dirty one keeps what the user typed, since an agent
 * writing the file is no reason to lose it, and says so when the file is no
 * longer what the editing began from: there is no merge, only Load theirs.
 */
export function mergeDisk(editor: Editor, disk: string): { text: string; conflict: boolean } {
  if (!editor.dirty) return { text: disk, conflict: false };
  return { text: editor.text, conflict: disk !== editor.base };
}
