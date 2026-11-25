/**
 * The toolset Start work offers for the repositories picked: the one they all
 * prefer, else the app's default, else Kiro. A toolset this Mac does not have
 * (a teammate's custom one) counts for nothing, as in the backend.
 */
export function suggestToolset(
  picked: { spec_toolset?: string | null }[],
  known: string[],
  appDefault: string | null,
): string {
  const mine = (id: string | null | undefined) => (id && known.includes(id) ? id : null);
  const wanted = picked.map((p) => mine(p.spec_toolset));
  const all = wanted.length > 0 && wanted.every((w) => w !== null && w === wanted[0]);
  return (all ? wanted[0] : null) ?? mine(appDefault) ?? "kiro";
}
