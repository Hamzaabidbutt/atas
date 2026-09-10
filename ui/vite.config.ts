import { defineConfig } from "vite";

export default defineConfig({
  // Tauri serves the built assets from a file:// origin, so every asset
  // reference has to be relative rather than rooted at /.
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: true,
  },
  server: { port: 5173, strictPort: true },
});
