// The settings window (ADR-0057). It talks to the same Rust state; the main window follows
// what changes here through `profile://changed`.
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
import "./styles/settings.css";

import { SettingsApp } from "./ui/settings/SettingsApp.tsx";

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from settings.html");

suppressNativeContextMenu();
createRoot(root).render(
  <StrictMode>
    <SettingsApp />
  </StrictMode>,
);
