import type { DraftImage, RepoResult } from "./types";

export type OpenPrRow = RepoResult & { image_target?: string };

export function draftPrImages(
  images: DraftImage[],
  checkouts: { id: string; project_name: string }[],
  opened: OpenPrRow[],
) {
  const targets: Record<string, string> = {};
  const eligible = images.filter((image) => {
    const name = image.repo?.trim().toLowerCase();
    const checkout = name
      ? checkouts.find((c) => c.project_name.toLowerCase() === name)
      : checkouts.length === 1
        ? checkouts[0]
        : undefined;
    const result = opened.find((r) => r.checkout_id === checkout?.id && r.ok);
    if (!result?.image_target) return false;
    targets[result.checkout_id] = result.image_target;
    return true;
  });
  return { images: eligible, targets };
}
