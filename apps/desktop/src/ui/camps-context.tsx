import { createContext, useContext, type ReactNode } from "react";
import type { CampsController, CampsState } from "../domain/camps.ts";
import { useStore } from "../lib/use-store.ts";

const CampsContext = createContext<CampsController | null>(null);

export function CampsProvider({ controller, children }: { controller: CampsController; children: ReactNode }) {
  return <CampsContext.Provider value={controller}>{children}</CampsContext.Provider>;
}

export function useCampsController(): CampsController {
  const controller = useContext(CampsContext);
  if (!controller) throw new Error("useCampsController must be used inside <CampsProvider>");
  return controller;
}

/** Subscribes to a slice of camps state. See `useStore` for selector rules. */
export function useCamps<T>(selector: (state: CampsState) => T): T {
  return useStore(useCampsController().store, selector);
}
