/**
 * The Henchman's standing orders (HEN-7, HEN-8) for the mock harness: two
 * orders and a chat left alone in a world with tasks, none in the empty one.
 */
import type { HenchmanOrders } from "../lib/types-henchman";

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
  return {
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
