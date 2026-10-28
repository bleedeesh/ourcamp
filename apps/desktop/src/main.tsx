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

import { CampsController, NO_CAMP } from "./domain/camps.ts";
import { DEMO_CAMP, DEMO_SELF_ID } from "./domain/demo-camp.ts";
import { VoiceController } from "./domain/voice.ts";
import { VolumeBook } from "./domain/volume.ts";
import type { CampApi } from "./engine/camp-api.ts";
import { DemoCampApi } from "./engine/demo-camp-api.ts";
import { DemoEngine } from "./engine/demo-engine.ts";
import { TauriCampApi } from "./engine/tauri-camp-api.ts";
import { isTauri, TauriEngine } from "./engine/tauri-engine.ts";
import { copyText } from "./lib/clipboard.ts";
import { App } from "./ui/App.tsx";
import { CampsProvider } from "./ui/camps-context.tsx";
import { VoiceProvider } from "./ui/voice-context.tsx";

const desktop = isTauri();
const engine = desktop ? new TauriEngine() : new DemoEngine(DEMO_SELF_ID);
const campApi: CampApi = desktop ? new TauriCampApi() : new DemoCampApi();
const voice = new VoiceController(engine, new VolumeBook(window.localStorage), NO_CAMP, "");
const camps = new CampsController(campApi, voice, {
  copy: copyText,
  // In the browser the demo camp brings its people, so the tree has something to show.
  ...(desktop
    ? {}
    : {
        toCamp: (open) => ({
          ...DEMO_CAMP,
          members: { ...DEMO_CAMP.members, [DEMO_SELF_ID]: { ...DEMO_CAMP.members[DEMO_SELF_ID]!, name: open.self.name } },
        }),
      }),
});
voice.start();
void camps.start();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from index.html");

createRoot(root).render(
  <StrictMode>
    <CampsProvider controller={camps}>
      <VoiceProvider controller={voice}>
        <App />
      </VoiceProvider>
    </CampsProvider>
  </StrictMode>,
);
