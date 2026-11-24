/** A spec file's editor, as far as a change on disk needs to know it. */
export interface Editor {
  text: string;
  /** The editor holds text that is not in the file. */
  dirty: boolean;
  /** What the file held when the editing began. */
  base: string;
}

/**
 * What an editor shows once the file on disk is read again (SPEC-28). A clean
 * editor takes the file. A dirty one keeps what the user typed, since an agent
 * writing the file is no reason to lose it, and says so when the file is no
 * longer what the editing began from: there is no merge, only Load theirs.
 */
export function mergeDisk(editor: Editor, disk: string): { text: string; conflict: boolean } {
  if (!editor.dirty) return { text: disk, conflict: false };
  return { text: editor.text, conflict: disk !== editor.base };
}
