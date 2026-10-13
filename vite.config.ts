import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and no screen clearing.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/crates/**"] } },
  envPrefix: ["VITE_"],
  build: { target: "es2021", sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
