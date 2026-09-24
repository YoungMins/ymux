// This package has no @types/node — tsconfig.json's "types": [] is
// deliberate, the frontend is browser code. shortcutList.test.ts is the one
// test that reads a source file off disk (to check SHORTCUTS against
// main.ts's keydown handler per CLAUDE.md rule 6), so this declares just the
// two Node APIs it needs rather than pulling in @types/node for the whole
// project. Vitest runs on Node, so "node:fs"/"node:url" resolve for real at
// runtime; this file is compile-time-only and must stay a global ambient
// declaration file (no imports/exports of its own — adding one turns these
// into module *augmentations*, which require the module to already resolve
// and fail to compile).

declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}

declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
