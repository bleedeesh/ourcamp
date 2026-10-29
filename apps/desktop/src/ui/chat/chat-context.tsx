import { createContext, useContext, type ReactNode } from "react";
import type { ChatController, ChatWindowState } from "../../domain/chat-window.ts";
import { useStore } from "../../lib/use-store.ts";

const ChatContext = createContext<ChatController | null>(null);

export function ChatProvider({ controller, children }: { controller: ChatController; children: ReactNode }) {
  return <ChatContext.Provider value={controller}>{children}</ChatContext.Provider>;
}

export function useChatController(): ChatController {
  const controller = useContext(ChatContext);
  if (!controller) throw new Error("useChatController must be used inside <ChatProvider>");
  return controller;
}

/** Subscribes to a slice of the chat window's state. See `useStore` for selector rules. */
export function useChat<T>(selector: (state: ChatWindowState) => T): T {
  return useStore(useChatController().store, selector);
}
