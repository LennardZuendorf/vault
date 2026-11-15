/**
 * The Henchman's standing orders (HEN-7, HEN-8) and its lair (HEN-13,
 * HEN-14) for the mock harness: two orders, a chat left alone, two needs
 * and what it did, in a world with tasks; none of it in the empty one.
 */
import type { HenchmanLair, HenchmanOrders } from "../lib/types-henchman";

type Args = Record<string, unknown>;

export function henchmanAnswers(seeded: boolean) {
  let next = 3;
  const state: HenchmanOrders = seeded
    ? {
        orders: [
          { id: "o1", text: "Yes to running the tests and the linter, in any task." },
          {
            id: "o2",
            text: "Asked whether to open a draft pull request: yes, as a draft, never ready for review.",
          },
        ],
        left_alone: [
          {
            key: "/Users/you/.villain-worktrees/_chat/c0ffee00",
            name: "Chat · Why is staging slow",
          },
        ],
      }
    : { orders: [], left_alone: [] };
  const now = () => structuredClone(state);
  const ago = (min: number) => new Date(Date.now() - min * 60_000).toISOString();
  const pane = "henchman";
  const lair: HenchmanLair = seeded
    ? {
        needs: [
          {
            id: "n1",
            at: ago(48),
            paneId: pane,
            taskId: "t-audit",
            key: "ACME-130",
            text: "Run `aws sso login --profile acme-ci`: the CI sign-in expired, so its checks can't run.",
            seen: true,
          },
          {
            id: "n2",
            at: ago(12),
            paneId: pane,
            taskId: "t-login",
            key: "ACME-123",
            text: "OK to post its reply to the high-severity review comment on #34? It disagrees with the reviewer.",
            seen: false,
          },
        ],
        did: [
          ["t-login", "ACME-123", 6, "put its agent on it: answer the 2 review comments on #34"],
          ["t-release", "ACME-141", 21, "told its agent: Go on with step 3 of your plan."],
          ["t-audit", "ACME-130", 33, "restarted its agent, which had exited"],
          [null, null, 40, "kept your order: Yes to running the tests and the linter."],
          ["t-login", "ACME-123", 55, "started its agent, as you asked: Fix the failing lint."],
          ["t-release", "ACME-141", 70, "told its agent: Yes, keep the notes."],
          ["t-audit", "ACME-130", 95, "picked its agent up: Go on where you were."],
        ].map(([taskId, key, min, what]) => ({
          taskId: taskId as string | null,
          key: key as string | null,
          at: ago(min as number),
          what: what as string,
        })),
      }
    : { needs: [], did: [] };
  return {
    henchman_lair: () => structuredClone(lair),
    henchman_need_done: (a: Args) => {
      lair.needs = lair.needs.filter((n) => n.id !== a.id);
      return structuredClone(lair);
    },
    henchman_orders: now,
    henchman_order_add: (a: Args) => {
      const text = String(a.text).trim();
      if (!text) throw new Error("an order needs words");
      if (!state.orders.some((o) => o.text === text)) state.orders.push({ id: `o${next++}`, text });
      return now();
    },
    henchman_order_remove: (a: Args) => {
      const at = state.orders.findIndex((o) => o.id === a.id);
      if (at < 0) throw new Error("no standing order has that id");
      state.orders.splice(at, 1);
      return now();
    },
    henchman_leave_alone: (a: Args) => {
      state.left_alone = state.left_alone.filter((x) => x.key !== a.key);
      return now();
    },
  };
}
