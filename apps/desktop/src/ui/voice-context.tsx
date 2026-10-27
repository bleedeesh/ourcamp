import { createContext, useContext, type ReactNode } from "react";
import type { VoiceController, VoiceState } from "../domain/voice.ts";
import { useStore } from "../lib/use-store.ts";

const VoiceContext = createContext<VoiceController | null>(null);

export function VoiceProvider({ controller, children }: { controller: VoiceController; children: ReactNode }) {
  return <VoiceContext.Provider value={controller}>{children}</VoiceContext.Provider>;
}

export function useVoiceController(): VoiceController {
  const controller = useContext(VoiceContext);
  if (!controller) throw new Error("useVoiceController must be used inside <VoiceProvider>");
  return controller;
}

/** Subscribes to a slice of voice state. See `useStore` for selector rules. */
export function useVoice<T>(selector: (state: VoiceState) => T): T {
  return useStore(useVoiceController().store, selector);
}
