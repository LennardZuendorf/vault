import { create } from "zustand";
import { api } from "./api";
import type { HenchmanOrders } from "./types-henchman";

/**
 * The Henchman's standing orders and what it leaves alone (HEN-7, HEN-8),
 * as the lair shows them. Its tools change them too, so they are read
 * again on `henchman:orders` (`Watchers.tsx`).
 */
interface Orders {
  orders: HenchmanOrders | null;
  refresh: () => Promise<void>;
  /** What a change returned: the lists after it. */
  set: (orders: HenchmanOrders) => void;
}

export const useOrders = create<Orders>((set) => ({
  orders: null,
  refresh: async () => set({ orders: await api.henchmanOrders() }),
  set: (orders) => set({ orders }),
}));
