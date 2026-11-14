import { describe, expect, it, vi } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { bumpInput, inputGeneration } from "./inputGen";

const { api } = await import("../ipc/bridge");
const core = await import("@tauri-apps/api/core");

const P = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const Q = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

describe("input generation", () => {
  it("counts per pane, from zero, and never goes back", () => {
    const fresh = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    expect(inputGeneration(fresh)).toBe(0);
    bumpInput(fresh);
    bumpInput(fresh);
    expect(inputGeneration(fresh)).toBe(2);
    expect(inputGeneration("dddddddd-dddd-4ddd-8ddd-dddddddddddd")).toBe(0);
  });

  it("advances on every api.writePane, before the write is issued, even a failing one", async () => {
    const before = inputGeneration(P);
    const seenAtInvoke: number[] = [];
    vi.mocked(core.invoke).mockImplementationOnce(async () => {
      seenAtInvoke.push(inputGeneration(P));
    });
    await api.writePane(P, new Uint8Array([49]));
    expect(seenAtInvoke).toEqual([before + 1]);
    vi.mocked(core.invoke).mockRejectedValueOnce(new Error("pty gone"));
    await expect(api.writePane(P, new Uint8Array([50]))).rejects.toThrow();
    expect(inputGeneration(P)).toBe(before + 2);
    // Another pane's counter is its own.
    const q = inputGeneration(Q);
    await api.writePane(P, new Uint8Array([51]));
    expect(inputGeneration(Q)).toBe(q);
  });
});

// ── One choke point ───────────────────────────────────────────────────────
// Every write the page makes to a terminal ends in `api.writePane`, the only
// caller of the `write_pane` command. This reads the sources to keep it so
// (a second path would let a menu id survive input it never saw).

const srcDir = fileURLToPath(new URL("..", import.meta.url));

function sources(): { path: string; text: string }[] {
  return readdirSync(srcDir, { recursive: true })
    .map((p) => p.replace(/\\/g, "/"))
    .filter((p) => p.endsWith(".ts") && !p.endsWith(".test.ts") && !p.endsWith(".d.ts"))
    .map((p) => ({ path: p, text: readFileSync(`${srcDir}/${p}`, "utf8") }));
}

describe("the write path", () => {
  it("has exactly one caller of the write_pane command, and it bumps first", () => {
    const callers = sources().filter((s) => /["'`]write_pane["'`]/.test(s.text));
    expect(callers.map((c) => c.path)).toEqual(["ipc/bridge.ts"]);
    const bridge = callers[0].text;
    const at = bridge.indexOf("writePane: async");
    expect(at).toBeGreaterThan(0);
    const body = bridge.slice(at, bridge.indexOf("resizePane", at));
    expect(body.indexOf("bumpInput(id)")).toBeGreaterThan(-1);
    expect(body.indexOf("bumpInput(id)")).toBeLessThan(body.indexOf("tauriInvoke("));
  });

  // The paths found (each reaches api.writePane, so each bumps): this table is
  // the list the generation covers. Adding a caller somewhere else fails here
  // on purpose: look at whether it can type into a terminal.
  const PATHS: Record<string, string> = {
    "terminal/TerminalPane.ts": "xterm keystrokes and IME, paste, drop, startup command, sendText, broadcast",
    "terminal/HotKeyBar.ts": "the hot key bar's command and key lines",
  };

  it("lists every file that calls api.writePane", () => {
    const callers = sources()
      .filter((s) => s.path !== "ipc/bridge.ts" && /\bapi\.writePane\(/.test(s.text))
      .map((s) => s.path)
      .sort();
    expect(callers).toEqual(Object.keys(PATHS).sort());
  });

  it("reaches the terminal elsewhere only through typeText / sendText of a TerminalPane", () => {
    // WorkspaceManager (prompt queue, Office peek keys, approve-all, broadcast,
    // handoffs) and the remote ops type via the pane, never via the bridge.
    const typing = sources()
      .filter((s) => /\.typeText\(/.test(s.text))
      .map((s) => s.path)
      .sort();
    expect(typing).toEqual(
      ["remote/remoteOps.ts", "terminal/TerminalPane.ts", "workspace/WorkspaceManager.ts"].sort(),
    );
    const pane = sources().find((s) => s.path === "terminal/TerminalPane.ts")!.text;
    const typeText = pane.slice(pane.indexOf("  typeText(text: string): void {"));
    expect(typeText.slice(0, 200)).toContain("api.writePane(");
  });

  it("is also advanced by every automatic answer (remoteWiring)", () => {
    const wiring = sources().find((s) => s.path === "remote/remoteWiring.ts")!.text;
    expect(wiring).toMatch(/onAutoAnswer\(\(ev\) => bumpInput\(ev\.paneId\)\)/);
  });
});
