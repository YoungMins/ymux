import { defineConfig } from "vite";
import { configDefaults } from "vitest/config";

// Agent git worktrees are full copies of this repo. Without these excludes
// vitest runs every test once per worktree and the dev server watches them.
const WORKTREES = "**/.claude/worktrees/**";

// Tauri expects a fixed dev-server port and the frontend built to ./dist
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: [WORKTREES] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: "esbuild",
    sourcemap: false,
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    exclude: [...configDefaults.exclude, WORKTREES],
  },
});
