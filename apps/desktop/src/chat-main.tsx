// The chat window (ADR-0046): a page of its own beside the main window. It talks to the
// same Rust state; the camp it shows is the one open in the main window.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { suppressNativeContextMenu } from "@ourcamp/ui/context-menu.ts";

import "@fontsource/onest/400.css";
import "@fontsource/onest/500.css";
import "@fontsource/onest/600.css";
import "@fontsource/onest/700.css";
import "@ourcamp/ui/tokens.css";
import "@ourcamp/ui/base.css";
import "./styles/app.css";
import "./styles/chat.css";

import { ChatController } from "./domain/chat-window.ts";
import type { CampApi } from "./engine/camp-api.ts";
import { DemoCampApi } from "./engine/demo-camp-api.ts";
import { TauriCampApi } from "./engine/tauri-camp-api.ts";
import { isTauri } from "./engine/tauri-engine.ts";
import { AvatarSourceContext, type AvatarSource } from "./ui/avatar-url.ts";
import { ChatApp } from "./ui/chat/ChatApp.tsx";
import { ChatProvider } from "./ui/chat/chat-context.tsx";

const api: CampApi = isTauri() ? new TauriCampApi() : new DemoCampApi();
const chat = new ChatController(api, window.localStorage);
void chat.start();
const avatarSource: AvatarSource = (userId, hash) => chat.avatarOf(userId, hash);

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from chat.html");

suppressNativeContextMenu();
createRoot(root).render(
  <StrictMode>
    <ChatProvider controller={chat}>
      <AvatarSourceContext.Provider value={avatarSource}>
        <ChatApp />
      </AvatarSourceContext.Provider>
    </ChatProvider>
  </StrictMode>,
);
