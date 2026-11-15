import { create } from "zustand";
import { api } from "./api";
import type { HenchmanLair } from "./types-henchman";

/**
 * The Henchman's open needs and what it did for the user (HEN-13, HEN-14),
 * kept by the backend across a restart. Read once, then replaced by each
 * `henchman:lair`, which carries the whole lair (`Watchers.tsx`).
 */
interface LairStore {
  lair: HenchmanLair | null;
  refresh: () => Promise<void>;
  set: (lair: HenchmanLair) => void;
}

export const useLair = create<LairStore>((set) => ({
  lair: null,
  refresh: async () => set({ lair: await api.henchmanLair() }),
  set: (lair) => set({ lair }),
}));
