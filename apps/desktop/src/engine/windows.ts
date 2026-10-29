import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "./tauri-engine.ts";

/**
 * Opens the chat window (ADR-0046), or brings it forward. In the browser demo it is a
 * popup of `chat.html` with its own demo camp.
 */
export async function openChatWindow(): Promise<void> {
  if (isTauri()) {
    await invoke("chat_window_open");
    return;
  }
  window.open("/chat.html", "ourcamp-chat", "popup,width=420,height=660");
}
