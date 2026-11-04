import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed port and its own error output. The tests mount
// components, so they take Svelte's browser build.
export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  clearScreen: false,
  resolve: mode === "test" ? { conditions: ["browser"] } : undefined,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
    // The views the Rust tests snapshot, which the component tests draw.
    fs: { allow: [".", "../tests/it/snapshots"] },
  },
  build: {
    target: "safari15",
    outDir: "dist",
  },
  test: {
    environment: "jsdom",
  },
}));
