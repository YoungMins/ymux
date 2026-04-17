// Wraps a single xterm.js Terminal + its addons and bridges stdin/stdout with
// the Rust PTY session via `api.spawnPane`, `api.writePane`, `onPaneData`.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { HotKeyDef, PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api, describeError, onPaneData, onPaneExit } from "../ipc/bridge";
import { HotKeyBar } from "./HotKeyBar";

export interface TerminalPaneOptions {
  spec: PaneSpec;
  /// Called when the child exits so the shell can be annotated in the UI.
  onExit?: (code: number) => void;
  /// Called when the user focuses this pane (via pointerdown or key).
  onFocus?: () => void;
  /// Called when the user mutates the HotKey list (add / edit / delete /
  /// reorder) so the owner can persist the new list into the PaneSpec.
  onHotKeysChange?: (hotkeys: HotKeyDef[]) => void;
}

/// Encodes a JS string into UTF-8 bytes for the PTY write pipe. ConPTY expects
/// the shell's native encoding; for PowerShell/pwsh/cmd/Git Bash that's UTF-8
/// as long as the shell's input codepage is configured accordingly, which is
/// the default on modern Windows Terminal.
const ENCODER = new TextEncoder();

export class TerminalPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  private termHost: HTMLElement;
  private hotkeyBar: HotKeyBar;
  private titleEl: HTMLElement;
  private term: Terminal;
  private fit: FitAddon;
  private search: SearchAddon;
  private searchBar: HTMLElement | null = null;
  private searchInput: HTMLInputElement | null = null;
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
    // Tag the element so a host-level focusin handler can find it via
    // `event.target.closest('.pane')` and update the focused pane id without
    // having to thread an `onFocus` callback through every render.
    this.element.dataset.paneId = this.id;

    // Title label shown above the hotkey bar. Falls back to the shell name
    // when no user title has been set (via `Ctrl+Shift+R`).
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.titleEl.textContent = opts.spec.title || opts.spec.shell || "pane";
    this.element.appendChild(this.titleEl);

    // Mount the HotKeyBar above xterm. An empty hotkey list still renders a
    // visible ⚙ button so the user can discover the feature.
    this.hotkeyBar = new HotKeyBar({
      paneId: this.id,
      initial: opts.spec.hotkeys ?? [],
      onChange: (next) => {
        this.spec = { ...this.spec, hotkeys: next };
        this.opts.onHotKeysChange?.(next);
      },
    });
    this.element.appendChild(this.hotkeyBar.element);

    // xterm mounts into a child element (not `this.element` directly) so the
    // HotKeyBar sibling doesn't get clobbered when xterm rearranges its
    // internal DOM subtree.
    this.termHost = document.createElement("div");
    this.termHost.className = "pane__term";
    this.element.appendChild(this.termHost);

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
    this.search = new SearchAddon();
    this.term.loadAddon(this.search);

    // Block xterm.js from consuming ymux-level hotkeys. Without this, Ctrl+F
    // etc. get translated into control bytes (Ctrl+F → 0x06) and written to
    // the PTY, never reaching our window keydown listener. Returning `false`
    // tells xterm to skip its own handling; the DOM event still bubbles up.
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown") return true;
      if (ev.ctrlKey && !ev.altKey) {
        const k = ev.key.toLowerCase();
        if (!ev.shiftKey && k === "f") return false;
        if (ev.shiftKey && (k === "h" || k === "v" || k === "w" || k === "z" || k === "r")) return false;
        if (k === "tab") return false;
      }
      if (ev.ctrlKey && ev.altKey && /^Digit[1-9]$/.test(ev.code)) return false;
      return true;
    });
    // Custom link handler: Ctrl+click opens the URL in the system browser via
    // the Rust backend instead of the default WebLinksAddon behaviour (which
    // tries `window.open` — unreliable inside WebView2).
    this.term.loadAddon(
      new WebLinksAddon((ev, uri) => {
        if (ev.ctrlKey) {
          ev.preventDefault();
          void api.openUrl(uri).catch((e) =>
            console.warn("openUrl failed:", e),
          );
        }
      }),
    );
    this.term.open(this.termHost);

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

    // `focusin` bubbles, unlike `focus`, so we catch the case where xterm.js
    // moves focus into its hidden helper textarea (a descendant of
    // `this.element`). `focus` would only fire if `this.element` itself
    // received focus, which never happens once xterm is inside it.
    this.element.addEventListener("focusin", () => this.opts.onFocus?.());
    // Pointerdown still routes clicks on the surrounding padding (outside
    // xterm's drawing area) into focus().
    this.element.addEventListener("pointerdown", () => this.focus());
  }

  async spawn(): Promise<void> {
    if (this.spawned) return;
    // Fit *synchronously* before reading dims so the PTY is spawned with the
    // actual rendered size instead of xterm.js's default 80×24. `scheduleFit`
    // (which queues a RAF) would race against `currentDims()` below and
    // produce 80×24, forcing a resize shortly after spawn — harmless for
    // plain cmd, but lethal for TUI apps like Claude Code that use
    // cursor-based in-place redraws: they compute their internal model at
    // 80 cols and xterm then renders at the actual width, and the two go
    // out of sync causing visible text overlap when the menu redraws.
    //
    // The `.pane` element is already attached to the DOM (WorkspaceManager
    // calls `renderWorkspace` before `spawn`), so `fit()` can compute real
    // dimensions from layout. If fit still throws (zero-size parent), fall
    // through to the defaults — the subsequent resize observer will correct
    // it, and plain shells won't care.
    try {
      this.fit.fit();
    } catch {
      // element not yet measurable; ignore
    }
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

  /// Toggle the search bar. Once shown, pressing Enter calls `findNext`,
  /// Shift+Enter calls `findPrevious`, Esc hides it. Multiple panes each get
  /// their own independent bar.
  toggleSearch(): void {
    if (!this.searchBar) this.buildSearchBar();
    const bar = this.searchBar!;
    const visible = bar.classList.toggle("search-bar--visible");
    if (visible) {
      this.searchInput!.focus();
      this.searchInput!.select();
    } else {
      // Restore the selection state so the user sees their highlight clear
      // cleanly. xterm's SearchAddon.clearDecorations exists in recent
      // versions; guard in case.
      (this.search as unknown as { clearDecorations?: () => void })
        .clearDecorations?.();
      this.term.focus();
    }
  }

  private buildSearchBar(): void {
    const bar = document.createElement("div");
    bar.className = "search-bar";

    const input = document.createElement("input");
    input.type = "text";
    input.className = "search-bar__input";
    input.placeholder = "Find…";
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        const opts = { incremental: false };
        if (ev.shiftKey) this.search.findPrevious(input.value, opts);
        else this.search.findNext(input.value, opts);
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        this.toggleSearch();
      }
    });

    const prevBtn = document.createElement("button");
    prevBtn.type = "button";
    prevBtn.className = "search-bar__btn";
    prevBtn.textContent = "↑";
    prevBtn.title = "Previous (Shift+Enter)";
    prevBtn.addEventListener("click", () =>
      this.search.findPrevious(input.value, { incremental: false }),
    );

    const nextBtn = document.createElement("button");
    nextBtn.type = "button";
    nextBtn.className = "search-bar__btn";
    nextBtn.textContent = "↓";
    nextBtn.title = "Next (Enter)";
    nextBtn.addEventListener("click", () =>
      this.search.findNext(input.value, { incremental: false }),
    );

    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.className = "search-bar__btn";
    closeBtn.textContent = "✕";
    closeBtn.title = "Close (Esc)";
    closeBtn.addEventListener("click", () => this.toggleSearch());

    bar.appendChild(input);
    bar.appendChild(prevBtn);
    bar.appendChild(nextBtn);
    bar.appendChild(closeBtn);
    this.termHost.appendChild(bar);
    this.searchBar = bar;
    this.searchInput = input;
  }

  /// Set the visible title for this pane. Used by the rename flow; the new
  /// title is also written back into the PaneSpec by WorkspaceManager.
  setTitle(title: string | null): void {
    this.spec = { ...this.spec, title };
    this.titleEl.textContent = title || this.spec.shell || "pane";
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
