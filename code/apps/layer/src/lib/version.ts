/**
 * What Settings ends with (SET-11). A release says its number. A build made
 * on this Mac (`bun run release:mac`) says the commit it was built from,
 * with `-dirty` when the tree had changes nobody committed: every local build
 * called itself the last release's number, so "is my fix in it?" had no
 * answer on screen.
 */
export function versionLine(
  version: string,
  build: string | undefined,
): { text: string; title: string } {
  const crash = "A crash is written to ~/Library/Logs/villain-layer/panic.log.";
  const commit = build?.trim();
  if (!commit) return { text: `Layer ${version}`, title: `Name this in a bug report. ${crash}` };
  return {
    text: `Layer ${commit}`,
    title: `Built on this Mac from commit ${commit}, after release ${version}. ${crash}`,
  };
}
