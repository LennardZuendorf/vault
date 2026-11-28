import { expect, test } from "bun:test";
import { taskCanFinish, taskReview } from "../src/lib/derive";
import { facts } from "../src/lib/lair";
import type { CheckoutPr } from "../src/lib/types";

test("a partial PR read cannot classify the task as merged", () => {
  const merged: CheckoutPr = {
    checkout_id: "c",
    repo: "web",
    pin: null,
    pin_revision: "0",
    pin_read: false,
    action_target: null,
    branch_covered: true,
    detached_unlanded: false,
    covered_merge: true,
    pr: {
      number: 17,
      title: "Change",
      state: "closed",
      merged: true,
      draft: false,
      author: "dev",
      head: "topic",
      head_sha: "a".repeat(40),
      base: "main",
      url: "https://github.com/acme/web/pull/17",
      mergeable_state: null,
      comments: 0,
      review_comments: 0,
    },
    checks: [],
    reviews: [],
    past: [],
    verdict: "none",
    base: "main",
    changed: 0,
    to_reply: 0,
    error: null,
  };
  const error = "GitHub pull requests exceed the read limit.";
  expect(taskReview([merged])).toBe("merged");
  expect(taskReview([{ ...merged, error }])).toBe("incomplete");
  expect(
    taskReview([merged, { ...merged, checkout_id: "other", repo: "api", pr: null, error }]),
  ).toBe("incomplete");
  expect(facts([], [{ ...merged, error }], 0).merged).toBe(false);
  const later = { ...merged, changed: 1, branch_covered: false, detached_unlanded: false };
  expect(taskReview([later])).toBe("incomplete");
  expect(taskCanFinish([later])).toBe(true);
  expect(taskCanFinish([{ ...later, detached_unlanded: true }])).toBe(false);
  expect(taskCanFinish([{ ...later, error }])).toBe(false);
  expect(taskCanFinish([{ ...later, covered_merge: false }])).toBe(false);
  expect(
    taskCanFinish([{ ...later, past: [{ ...merged.pr!, merged: false, state: "open" }] }]),
  ).toBe(false);
  expect(taskReview([{ ...merged, covered_merge: false }])).toBe("incomplete");
  expect(taskReview([{ ...merged, past: [{ ...merged.pr!, merged: false, state: "open" }] }])).toBe(
    "open",
  );
});
