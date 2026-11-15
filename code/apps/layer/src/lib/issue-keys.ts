/**
 * Ticket keys in what an agent says (ACP-18). A word is a key only when its
 * project is one Layer knows, so `SHA-256`, `UTF-8` and `ISO-8601` stay words.
 */

// Jira's own rule for a project key: a capital, then capitals or digits.
const KEY = /\b([A-Z][A-Z0-9]{0,9})-\d+\b/g;

/**
 * The projects Layer knows, as one string ("ACME|OPS"), so a selector that
 * returns it changes only when a project comes or goes.
 */
export function keyProjects(keys: (string | null | undefined)[]): string {
  const projects = new Set<string>();
  for (const k of keys) {
    const at = k ? k.search(/-\d+$/) : -1;
    if (k && at > 0 && /^[A-Z][A-Z0-9]*$/.test(k.slice(0, at))) projects.add(k.slice(0, at));
  }
  return [...projects].sort().join("|");
}

/** `text` with each key of a known project set apart, in order. */
export function splitKeys(text: string, projects: string): (string | { key: string })[] {
  if (!projects) return [text];
  const known = new Set(projects.split("|"));
  const out: (string | { key: string })[] = [];
  let from = 0;
  for (const m of text.matchAll(KEY)) {
    if (!known.has(m[1])) continue;
    if (m.index > from) out.push(text.slice(from, m.index));
    out.push({ key: m[0] });
    from = m.index + m[0].length;
  }
  if (from < text.length) out.push(text.slice(from));
  return out;
}
