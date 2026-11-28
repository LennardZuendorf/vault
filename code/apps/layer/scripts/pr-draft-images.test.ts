import { expect, test } from "bun:test";
import { draftPrImages } from "../src/lib/pr-draft-images";

test("draft screenshots go only to checkouts whose exact PR was opened or reused", () => {
  const repos = [
    { id: "a", project_name: "API" },
    { id: "b", project_name: "web" },
    { id: "c", project_name: "infra" },
  ];
  const images = [
    { path: "/api.png", repo: "api", caption: "Night" },
    { path: "/web.png", repo: "web", caption: "Day" },
    { path: "/infra.png", repo: "infra", caption: "Night" },
    { path: "/unnamed.png", repo: null, caption: "Day" },
  ];
  const opened = [
    { checkout_id: "a", repo: "API", ok: true, detail: "#17", image_target: "approved-17" },
    { checkout_id: "b", repo: "web", ok: true, detail: "#23 already open on another head" },
  ];
  expect(draftPrImages(images, repos, opened)).toEqual({
    images: [images[0]],
    targets: { a: "approved-17" },
  });
  expect(draftPrImages([images[3]], [repos[0]], opened).images).toEqual([images[3]]);
});
