import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@fontsource/onest/400.css";
import "@fontsource/onest/500.css";
import "@fontsource/onest/600.css";
import "@fontsource/onest/700.css";
import "@fontsource/nunito/800.css";
import "@fontsource/fredoka/600.css";
import "@ourcamp/ui/tokens.css";
import "@ourcamp/ui/base.css";

import { Campfire } from "@ourcamp/ui/Campfire.tsx";

// The full window (status, links, members, tray) arrives later in M3 step 6.
function App() {
  return (
    <main style={{ display: "grid", placeItems: "center", height: "100%" }}>
      <Campfire size={64} />
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing from index.html");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
