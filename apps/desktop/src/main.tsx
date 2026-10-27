import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@fontsource/onest/400.css";
import "@fontsource/onest/500.css";
import "@fontsource/onest/600.css";
import "@fontsource/onest/700.css";
import "@fontsource/nunito/800.css";
import "@fontsource/fredoka/600.css";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";

import { DEMO_CAMP, DEMO_SELF_ID } from "./domain/demo-camp.ts";
import { VoiceController } from "./domain/voice.ts";
import { VolumeBook } from "./domain/volume.ts";
import { DemoEngine } from "./engine/demo-engine.ts";
import { isTauri, TauriEngine } from "./engine/tauri-engine.ts";
import { App } from "./ui/App.tsx";
import { VoiceProvider } from "./ui/voice-context.tsx";

const engine = isTauri() ? new TauriEngine() : new DemoEngine();
const voice = new VoiceController(engine, new VolumeBook(window.localStorage), DEMO_CAMP, DEMO_SELF_ID);
voice.start();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from index.html");

createRoot(root).render(
  <StrictMode>
    <VoiceProvider controller={voice}>
      <App />
    </VoiceProvider>
  </StrictMode>,
);
