// The viewer's window (ADR-0053): a page around the picture, which Rust draws by itself
// in the place this page leaves for it. No frame ever comes here.
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
import "./styles/stream.css";

import { StreamApp } from "./ui/stream/StreamApp.tsx";

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from stream.html");

suppressNativeContextMenu();
createRoot(root).render(
  <StrictMode>
    <StreamApp />
  </StrictMode>,
);
