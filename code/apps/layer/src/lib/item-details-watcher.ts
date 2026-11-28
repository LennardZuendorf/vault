import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useStore } from "../store";
import type { DetailsEvent } from "./types";

export function useItemDetails() {
  const fail = useStore((s) => s.fail);
  useEffect(() => {
    const p = listen<DetailsEvent>("item-details:changed", (e) => {
      void useStore.getState().detailsChanged(e.payload).catch(fail);
    });
    void useStore.getState().refreshTags().catch(fail);
    return () => {
      void p.then((unlisten) => unlisten());
    };
  }, [fail]);
}
