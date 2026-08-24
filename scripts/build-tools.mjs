// Build the ymux companion tools (ymon, ydir, ycode, ylauncher) and copy
// them into src-tauri/binaries/ with the target-triple suffix Tauri's
// externalBin requires. Called by `pnpm build` before `tauri build`.
//
// Usage: node scripts/build-tools.mjs

import { execSync } from "node:child_process";
import { existsSync, mkdirSync, copyFileSync, rmSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");

// Workspace members to bundle. Each entry: cargo package name → output binary
// basename (matches the [[bin]] name in the package's Cargo.toml).
const TOOLS = [
  { pkg: "ymon", bin: "ymon" },
  { pkg: "ydir", bin: "ydir" },
  { pkg: "ycode", bin: "ycode" },
  { pkg: "ylauncher", bin: "y" },
  { pkg: "ygit", bin: "ygit" },
];

function run(cmd) {
  console.log(`> ${cmd}`);
  execSync(cmd, { cwd: root, stdio: "inherit" });
}

function detectTargetTriple() {
  // An explicit triple wins. Tauri's `--target` and this script have to agree
  // — Tauri looks for sidecars suffixed with the triple it is building for,
  // so a mismatch surfaces as a confusing "sidecar not found" at bundle time
  // rather than at the point the two diverged. CI sets this once and passes
  // the same value to both.
  const explicit = process.env.YMUX_TARGET_TRIPLE;
  if (explicit) return explicit;
  try {
    const out = execSync("rustc -vV", { encoding: "utf-8" });
    const match = out.match(/^host:\s*(\S+)/m);
    if (match) return match[1];
  } catch {
    // fall through
  }
  // Fallback for Windows x64 — the only platform we ship MSIs for.
  return "x86_64-pc-windows-msvc";
}

const triple = detectTargetTriple();
const isWindows = triple.includes("windows");
const exeSuffix = isWindows ? ".exe" : "";

console.log(`Building tools for target: ${triple}`);
const args = TOOLS.map((t) => `-p ${t.pkg}`).join(" ");
run(`cargo build --release ${args}`);

const binariesDir = join(root, "src-tauri", "binaries");
if (existsSync(binariesDir)) {
  rmSync(binariesDir, { recursive: true, force: true });
}
mkdirSync(binariesDir, { recursive: true });

for (const tool of TOOLS) {
  const srcPath = join(root, "target", "release", `${tool.bin}${exeSuffix}`);
  if (!existsSync(srcPath)) {
    console.error(`expected ${srcPath} missing — cargo build did not produce it`);
    process.exit(1);
  }
  // Tauri externalBin: looks up `<basename>-<triple><.exe?>` and installs
  // it as `<basename><.exe?>` next to the main binary.
  const destPath = join(binariesDir, `${tool.bin}-${triple}${exeSuffix}`);
  copyFileSync(srcPath, destPath);
  if (!isWindows) {
    // Tauri copies sidecars into `ymux.app/Contents/MacOS/` verbatim. If the
    // executable bit is lost along the way the tools silently fail to launch
    // from a pane, so set it explicitly rather than trusting copy semantics.
    chmodSync(destPath, 0o755);
  }
  console.log(`copied ${srcPath} → ${destPath}`);
}

console.log(`✓ ${TOOLS.length} tools staged in ${binariesDir}`);
