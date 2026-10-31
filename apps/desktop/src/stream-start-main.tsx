// The window "Начать стрим" (ADR-0054): what to show, in what quality. It talks to the same
// Rust state; the camp it reads is the one open in the main window.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { suppressNativeContextMenu } from "@ourcamp/ui/context-menu.ts";

import "@fontsource/onest/400.css";
import "@fontsource/onest/500.css";
import "@fontsource/onest/600.css";
import "@fontsource/onest/700.css";
import "@fontsource/nunito/800.css";
import "@ourcamp/ui/tokens.css";
import "@ourcamp/ui/base.css";
import "./styles/app.css";
import "./styles/stream-start.css";

import { StreamStartApp } from "./ui/stream/StreamStartApp.tsx";

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from stream-start.html");

suppressNativeContextMenu();
createRoot(root).render(
  <StrictMode>
    <StreamStartApp />
  </StrictMode>,
);
