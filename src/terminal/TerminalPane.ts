// Wraps a single xterm.js Terminal + its addons and bridges stdin/stdout with
// the Rust PTY session via `api.spawnPane`, `api.writePane`, `onPaneData`.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import "@xterm/xterm/css/xterm.css";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { PaneSpec, Uuid } from "../types";
import { api, describeError, onPaneData, onPaneExit } from "../ipc/bridge";

export interface TerminalPaneOptions {
  spec: PaneSpec;
  /// Called when the child exits so the shell can be annotated in the UI.
  onExit?: (code: number) => void;
  /// Called when the user focuses this pane (via pointerdown or key).
  onFocus?: () => void;
}

/// Encodes a JS string into UTF-8 bytes for the PTY write pipe. ConPTY expects
/// the shell's native encoding; for PowerShell/pwsh/cmd/Git Bash that's UTF-8
/// as long as the shell's input codepage is configured accordingly, which is
/// the default on modern Windows Terminal.
const ENCODER = new TextEncoder();

export class TerminalPane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  private term: Terminal;
  private fit: FitAddon;
  private unlisteners: UnlistenFn[] = [];
  private spawned = false;
  private spec: PaneSpec;
  private opts: TerminalPaneOptions;
  private pendingResizeRaf = 0;

  constructor(opts: TerminalPaneOptions) {
    this.id = opts.spec.id;
    this.spec = opts.spec;
    this.opts = opts;

    this.element = document.createElement("div");
    this.element.className = "pane";
    this.element.tabIndex = 0;

    this.term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily:
        "Cascadia Code, Consolas, 'Courier New', ui-monospace, monospace",
      fontSize: 13,
      scrollback: 10_000,
      theme: {
        background: "#0b0f14",
        foreground: "#d6deeb",
        cursor: "#7fdbca",
        black: "#000000",
        red: "#ef6b73",
        green: "#8ae234",
        yellow: "#f3d64e",
        blue: "#7aa6da",
        magenta: "#c397d8",
        cyan: "#70c0ba",
        white: "#eaeaea",
      },
    });

    this.fit = new FitAddon();
    this.term.loadAddon(this.fit);
    this.term.loadAddon(new WebLinksAddon());
    this.term.open(this.element);

    this.term.onData((data) => {
      if (!this.spawned) return;
      const bytes = ENCODER.encode(data);
      void api.writePane(this.id, bytes);
    });

    this.term.onResize(({ cols, rows }) => {
      if (!this.spawned) return;
      void api.resizePane({
        id: this.id,
        rows,
        cols,
        pixelWidth: 0,
        pixelHeight: 0,
      });
    });

    this.element.addEventListener("pointerdown", () => this.focus());
    this.element.addEventListener("focus", () => this.opts.onFocus?.());
  }

  async spawn(): Promise<void> {
    if (this.spawned) return;
    // Defer fit until we are actually in the DOM with a nonzero size.
    this.scheduleFit();
    const { cols, rows } = this.currentDims();
    try {
      await api.spawnPane({
        id: this.id,
        shell: this.spec.shell,
        cwd: this.spec.cwd ?? null,
        rows,
        cols,
      });
      this.spawned = true;

      const dataUnlisten = await onPaneData(this.id, (bytes) => {
        this.term.write(bytes);
      });
      const exitUnlisten = await onPaneExit(this.id, (code) => {
        this.term.writeln(`\r\n\x1b[2m[process exited with code ${code}]\x1b[0m`);
        this.opts.onExit?.(code);
      });
      this.unlisteners.push(dataUnlisten, exitUnlisten);

      // Optional startup command: the Rust side intentionally does not run
      // this itself; the frontend knows when the terminal is actually ready
      // to accept input, which avoids races with the shell's own init
      // output.
      if (this.spec.startup_cmd) {
        setTimeout(() => {
          void api.writePane(
            this.id,
            ENCODER.encode(`${this.spec.startup_cmd}\r`),
          );
        }, 200);
      }
    } catch (e) {
      // `e` from Tauri can be a string (Rust error serialized as a string),
      // an Error (wrapped by `bridge.ts call()`), an object (capability
      // rejection), or even `undefined` if a permission was denied silently.
      // Render *something* useful in every case.
      const msg = describeError(e);
      this.term.writeln(`\x1b[31mfailed to start shell: ${msg}\x1b[0m`);
      throw e;
    }
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
    this.term.focus();
    this.opts.onFocus?.();
  }

  /// Recompute size based on the container. Debounced to one call per
  /// animation frame.
  scheduleFit(): void {
    if (this.pendingResizeRaf) return;
    this.pendingResizeRaf = requestAnimationFrame(() => {
      this.pendingResizeRaf = 0;
      try {
        this.fit.fit();
      } catch {
        // fit throws when the element has zero size; ignore.
      }
    });
  }

  private currentDims(): { cols: number; rows: number } {
    const cols = this.term.cols || 80;
    const rows = this.term.rows || 24;
    return { cols, rows };
  }

  dispose(): void {
    for (const u of this.unlisteners) u();
    this.unlisteners = [];
    if (this.spawned) {
      void api.killPane(this.id).catch(() => {});
    }
    this.term.dispose();
    this.element.remove();
  }
}
