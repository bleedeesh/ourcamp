import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { suppressNativeContextMenu } from "@ourcamp/ui/context-menu.ts";

import "@fontsource/onest/400.css";
import "@fontsource/onest/500.css";
import "@fontsource/onest/600.css";
import "@fontsource/onest/700.css";
import "@fontsource/nunito/800.css";
import "@fontsource/fredoka/600.css";
import "@ourcamp/ui/tokens.css";
import "@ourcamp/ui/base.css";
import "./styles/server.css";

import { DemoServerApi, isTauri, TauriServerApi } from "./api.ts";
import { App } from "./ui/App.tsx";

const api = isTauri() ? new TauriServerApi() : new DemoServerApi();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from index.html");


// Right clicks open our own menus; the browser's Back/Reload/Inspect is no part of the app.
suppressNativeContextMenu();
createRoot(root).render(
  <StrictMode>
    <App api={api} />
  </StrictMode>,
);
