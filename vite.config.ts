import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and must not clear the terminal it shares with cargo.
export default defineConfig(({ command }) => ({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "es2021", sourcemap: true, chunkSizeWarningLimit: 1500 },
  resolve: {
    // The browser-only mock backend must never ship in a production bundle.
    alias:
      command === "build"
        ? [{ find: /^\.\/devMock$/, replacement: fileURLToPath(new URL("./src/lib/devMock.stub.ts", import.meta.url)) }]
        : [],
  },
}));
