/**
 * The `@` a layout types with Option (PANE-8). With Option as Meta, xterm
 * would send Escape and the base key, and an agent's file picker never sees
 * the `@`. Option+⌘ and Option+Ctrl stay shortcuts.
 */
export function optionTypedAt(e: KeyboardEvent): boolean {
  return (
    e.type === "keydown" && e.key === "@" && e.altKey && !e.ctrlKey && !e.metaKey && !e.isComposing
  );
}
