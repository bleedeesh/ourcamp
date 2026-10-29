import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and must not have the browser opened for it.
// https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host ?? false,
    // Omitted (not `undefined`) without a host: `exactOptionalPropertyTypes` is on.
    ...(host ? { hmr: { protocol: "ws", host, port: 5174 } } : {}),
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // WebView2 (Windows) and WKWebView (macOS 13+) both handle modern syntax.
    target: "es2022",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    // Two pages: the main window and the chat window (ADR-0046).
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("index.html", import.meta.url)),
        chat: fileURLToPath(new URL("chat.html", import.meta.url)),
      },
    },
  },
});
