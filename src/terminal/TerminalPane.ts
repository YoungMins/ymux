// Wraps a single xterm.js Terminal + its addons and bridges stdin/stdout with
// the Rust PTY session via `api.spawnPane`, `api.writePane`, `onPaneData`.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import { CanvasAddon } from "@xterm/addon-canvas";
import { SerializeAddon } from "@xterm/addon-serialize";
import "@xterm/xterm/css/xterm.css";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { HotKeyDef, PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api, describeError, onPaneData, onPaneExit } from "../ipc/bridge";
import { HotKeyBar } from "./HotKeyBar";
import { t, onLangChange } from "../i18n/i18n";
import { PaneStatusMachine, type PaneStatus } from "./paneStatus";
import { restoreScrollGuard, restoreRevealLines } from "./restoreGuard";

export interface TerminalPaneOptions {
  spec: PaneSpec;
  /// Called when the child exits so the shell can be annotated in the UI.
  onExit?: (code: number) => void;
  /// Called when the user focuses this pane (via pointerdown or key).
  onFocus?: () => void;
  /// Called when the user mutates the HotKey list (add / edit / delete /
  /// reorder) so the owner can persist the new list into the PaneSpec.
  onHotKeysChange?: (hotkeys: HotKeyDef[]) => void;
  onBgColorChange?: (color: string | null) => void;
  /// Fired when the terminal emits a bell (BEL) or an OSC 9 notification —
  /// the signal a long-running CLI (claude/codex/gemini) uses to say it
  /// finished or needs attention. `message` is the OSC 9 text if present,
  /// else null.
  onAttention?: (message: string | null) => void;
  /// Fired whenever this pane's derived status (idle/running/done/attention)
  /// changes, so the owner can render a per-pane status indicator.
  onStatusChange?: (status: PaneStatus) => void;
  /// Returns whether scrollback persistence is currently enabled (read live
  /// from WorkspaceManager's toggle rather than snapshotted at pane-creation
  /// time, so flipping the setting takes effect immediately).
  persistScrollback?: () => boolean;
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
  private serializeAddon = new SerializeAddon();
  private searchBar: HTMLElement | null = null;
  private searchInput: HTMLInputElement | null = null;
  private unlisteners: UnlistenFn[] = [];
  private spawned = false;
  private spec: PaneSpec;
  private opts: TerminalPaneOptions;
  private pendingResizeRaf = 0;
  /// Lines to scroll up once the shell has painted its first output, to bring
  /// restored scrollback back into view. 0 = nothing to reveal.
  private pendingRestoreReveal = 0;
  private cleanupLang: () => void = () => {};
  private statusMachine = new PaneStatusMachine((s) => this.opts.onStatusChange?.(s));
  private isFocused = false;
  private statusTimer: number | undefined;
  private scrollbackSaveTimer: number | undefined;
  private flushScrollbackOnUnload = (): void => {
    if (this.opts.persistScrollback?.()) {
      void api.saveScrollback(this.id, this.serializeAddon.serialize());
    }
  };

  constructor(opts: TerminalPaneOptions) {
    this.id = opts.spec.id;
    this.spec = opts.spec;
    this.opts = opts;

    this.element = document.createElement("div");
    this.element.className = "pane";
    this.element.tabIndex = 0;
    if (opts.spec.bg_color) {
      this.element.style.background = opts.spec.bg_color;
    }
    // Tag the element so a host-level focusin handler can find it via
    // `event.target.closest('.pane')` and update the focused pane id without
    // having to thread an `onFocus` callback through every render.
    this.element.dataset.paneId = this.id;

    // Title label shown above the hotkey bar. Falls back to the shell name
    // when no user title has been set (via `Ctrl+Shift+R`).
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.titleEl.textContent = opts.spec.title || opts.spec.shell || t("terminal.defaultTitle");
    this.element.appendChild(this.titleEl);

    // Mount the HotKeyBar above xterm. An empty hotkey list still renders a
    // visible ⚙ button so the user can discover the feature.
    this.hotkeyBar = new HotKeyBar({
      paneId: this.id,
      initial: opts.spec.hotkeys ?? [],
      initialBgColor: opts.spec.bg_color ?? null,
      onChange: (next) => {
        this.spec = { ...this.spec, hotkeys: next };
        this.opts.onHotKeysChange?.(next);
      },
      onBgColorChange: (color) => {
        this.setBgColor(color);
        this.opts.onBgColorChange?.(color);
      },
    });
    this.element.appendChild(this.hotkeyBar.element);

    // xterm mounts into a child element (not `this.element` directly) so the
    // HotKeyBar sibling doesn't get clobbered when xterm rearranges its
    // internal DOM subtree.
    this.termHost = document.createElement("div");
    this.termHost.className = "pane__term";
    this.element.appendChild(this.termHost);

    const bgColor = opts.spec.bg_color || "#0b0f14";
    this.term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily:
        "Cascadia Code, Consolas, 'Courier New', ui-monospace, monospace",
      fontSize: 13,
      scrollback: 10_000,
      // Squish ambiguous-width glyphs that the OS fallback font draws
      // 2 cells wide back into their declared 1-cell slot, so they
      // don't overflow into the next cell and leave ghost remnants
      // after a redraw. Requires a non-DOM renderer (Canvas below).
      rescaleOverlappingGlyphs: true,
      theme: {
        background: bgColor,
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
    // Canvas renderer (not the default DOM one). xterm.js's
    // `rescaleOverlappingGlyphs` option is a no-op under DOM; Canvas
    // honors it and also tends to track per-cell repaints more
    // precisely than DOM for fast-redraw patterns (PSReadLine prompts,
    // ratatui paragraph redraws). We pick Canvas over WebGL because
    // WebGL caused a cell-positioning regression in v0.8.14.
    this.term.loadAddon(new CanvasAddon());

    // Block xterm.js from consuming ymux-level hotkeys. Without this, Ctrl+F
    // etc. get translated into control bytes (Ctrl+F → 0x06) and written to
    // the PTY, never reaching our window keydown listener. Returning `false`
    // tells xterm to skip its own handling; the DOM event still bubbles up.
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown") return true;
      if (ev.ctrlKey && !ev.altKey) {
        const k = ev.key.toLowerCase();
        // Ctrl+V → paste clipboard text into the PTY instead of
        // letting xterm send the raw 0x16 byte.
        if (!ev.shiftKey && k === "v") {
          ev.preventDefault();
          void this.pasteClipboard();
          return false;
        }
        if (!ev.shiftKey && k === "f") return false;
        if (ev.shiftKey && (k === "h" || k === "v" || k === "w" || k === "z" || k === "r" || k === "p")) return false;
        // Ctrl+Shift+Left/Right → swap pane position (handled at window level).
        if (ev.shiftKey && (k === "arrowleft" || k === "arrowright")) return false;
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
    // Serialize addon: snapshots the buffer (text + escape sequences) so it
    // can be replayed on next mount when scrollback persistence is enabled.
    this.term.loadAddon(this.serializeAddon);

    // Flush the current buffer to disk on app shutdown (normal window
    // close), *without* deleting it — that's what makes restore-on-mount
    // possible next launch. This is intentionally separate from `dispose()`,
    // which is only invoked from explicit user-close paths (kill pane /
    // delete workspace) and therefore deletes instead.
    window.addEventListener("beforeunload", this.flushScrollbackOnUnload);

    // Drive the derived idle/running/done/attention status from a 1s
    // ticker so `running` decays back to `idle` after a quiet period even
    // when no new output/input arrives to trigger a recheck.
    this.statusTimer = window.setInterval(
      () => this.statusMachine.tick(Date.now()),
      1000,
    );

    // Bell (BEL 0x07) → attention signal with no message.
    this.term.onBell(() => {
      opts.onAttention?.(null);
      this.statusMachine.onAttention(this.isFocused);
    });
    // OSC 9 → attention with the payload text as the message. Only the
    // iTerm2-style plain-text form (`OSC 9 ; <message>`) is a completion
    // notification. Windows Terminal / ConEmu reuse OSC 9 for progress
    // (`9;4;…`), cwd (`9;9;…`), etc., whose payload starts with "<digit>;" —
    // swallow those without alerting so long-running tools don't spam beeps.
    this.term.parser.registerOscHandler(9, (data) => {
      if (!/^\d+;/.test(data)) {
        opts.onAttention?.(data || null);
        this.statusMachine.onAttention(this.isFocused);
      }
      return true;
    });

    this.term.onData((data) => {
      if (data.includes("\r")) this.statusMachine.onSubmit(Date.now());
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
    this.element.addEventListener("pointerdown", () => this.focus());

    this.cleanupLang = onLangChange(() => this.updateLang());
  }

  private updateLang(): void {
    if (!this.spec.title && !this.spec.shell) {
      this.titleEl.textContent = t("terminal.defaultTitle");
    }
    if (this.searchInput) {
      this.searchInput.placeholder = t("terminal.findPlaceholder");
    }
    if (this.searchBar) {
      const btns = this.searchBar.querySelectorAll<HTMLButtonElement>(".search-bar__btn");
      if (btns[0]) btns[0].title = t("terminal.findPrev");
      if (btns[1]) btns[1].title = t("terminal.findNext");
      if (btns[2]) btns[2].title = t("terminal.findClose");
    }
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

    // Restore prior scrollback (if persistence is enabled and a save exists)
    // BEFORE the live PTY listener is registered below, so replayed history
    // always renders above anything the shell writes this session. A load
    // failure must never block spawn, hence the try/catch swallow.
    if (this.opts.persistScrollback?.()) {
      try {
        const prior = await api.loadScrollback(this.id);
        if (prior) {
          this.term.write(prior);
          this.term.write(`\r\n\x1b[2m${t("terminal.sessionRestored")}\x1b[0m\r\n`);
          // ConPTY opens every session by emitting `\x1b[2J\x1b[H` (clear
          // screen + home). That erases the viewport rows — and if the
          // restored history is shorter than the viewport, it lives entirely
          // in those rows and gets wiped, so the restore flashes in then
          // vanishes as if `cls` ran. Scroll the restored block up into the
          // scrollback ring (which `\x1b[2J` leaves untouched) first, so the
          // shell clears a blank viewport instead of the restored text.
          this.term.write(restoreScrollGuard(this.term.rows));
          // The guard keeps the history safe but parks it above the viewport,
          // so the pane opens showing only a bare prompt — indistinguishable
          // from "nothing was restored". Reveal it by scrolling up once the
          // shell has painted (see the data listener below).
          this.pendingRestoreReveal = restoreRevealLines(this.term.rows);
        }
      } catch {
        // No prior scrollback (or load failed) — start clean.
      }
    }

    // Register data + exit listeners *before* spawning the PTY. Tauri's
    // `emit` is fire-and-forget — events for `pty:data:{id}` that arrive
    // while no listener is registered are dropped on the floor. TUI apps
    // like Claude Code / Codex emit their alt-screen entry
    // (`\x1b[?1049h`), mouse-mode setup, and initial cursor positioning
    // immediately on start; missing any of those leaves xterm and the
    // shell in disagreement about screen state and shows up as garbled,
    // overlapping output ("화면이 깨진다") that never recovers until a
    // full redraw.
    const dataUnlisten = await onPaneData(this.id, (bytes) => {
      // The write callback fires once xterm has parsed this chunk, so a reveal
      // scheduled here happens strictly after the shell's opening burst (with
      // its `\x1b[2J` clear) has been applied — scrolling any earlier would be
      // undone by that clear.
      this.term.write(bytes, () => {
        if (this.pendingRestoreReveal > 0) {
          this.term.scrollLines(-this.pendingRestoreReveal);
          this.pendingRestoreReveal = 0;
        }
      });
      this.statusMachine.onOutput(Date.now());
      this.scheduleScrollbackSave();
    });
    const exitUnlisten = await onPaneExit(this.id, (code) => {
      this.term.writeln(`\r\n\x1b[2m[process exited with code ${code}]\x1b[0m`);
      this.opts.onExit?.(code);
    });
    this.unlisteners.push(dataUnlisten, exitUnlisten);

    try {
      await api.spawnPane({
        id: this.id,
        shell: this.spec.shell,
        cwd: this.spec.cwd ?? null,
        rows,
        cols,
      });
      this.spawned = true;

      // Re-apply background color after spawn — xterm may reset its
      // internal theme when the terminal size changes during fit().
      if (this.spec.bg_color) {
        this.setBgColor(this.spec.bg_color);
      }

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
      // Spawn failed — tear down the listeners we registered above so they
      // don't leak (and so a retry with the same pane id doesn't double-fire
      // the data handler).
      for (const u of this.unlisteners) u();
      this.unlisteners = [];
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
    this.isFocused = true;
    this.statusMachine.onFocus();
    this.element.focus({ preventScroll: true });
    this.term.focus();
    this.opts.onFocus?.();
  }

  /// Called when this pane loses focus (another pane is focused instead).
  /// There is no DOM blur event we can rely on here — xterm's hidden helper
  /// textarea moves focus around internally — so the owner (WorkspaceManager)
  /// calls this explicitly from its focused-pane tracking.
  blur(): void {
    this.isFocused = false;
  }

  get status(): PaneStatus {
    return this.statusMachine.status;
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
    input.placeholder = t("terminal.findPlaceholder");
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
    prevBtn.title = t("terminal.findPrev");
    prevBtn.addEventListener("click", () =>
      this.search.findPrevious(input.value, { incremental: false }),
    );

    const nextBtn = document.createElement("button");
    nextBtn.type = "button";
    nextBtn.className = "search-bar__btn";
    nextBtn.textContent = "↓";
    nextBtn.title = t("terminal.findNext");
    nextBtn.addEventListener("click", () =>
      this.search.findNext(input.value, { incremental: false }),
    );

    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.className = "search-bar__btn";
    closeBtn.textContent = "✕";
    closeBtn.title = t("terminal.findClose");
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
  setBgColor(color: string | null): void {
    const bg = color || "#0b0f14";
    this.spec = { ...this.spec, bg_color: color ?? "" };
    this.term.options.theme = { ...this.term.options.theme, background: bg };
    this.element.style.background = bg;
  }

  setTitle(title: string | null): void {
    this.spec = { ...this.spec, title };
    this.titleEl.textContent = title || this.spec.shell || t("terminal.defaultTitle");
  }

  /// Write literal text into the PTY as if the user had typed it — no
  /// trailing newline, so nothing is executed until they press Enter.
  /// Used by the file drag-and-drop handler to insert dropped paths.
  typeText(text: string): void {
    if (!this.spawned || !text) return;
    void api.writePane(this.id, ENCODER.encode(text));
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

  private async pasteClipboard(): Promise<void> {
    // Image first: if the clipboard holds a PNG, save it to a temp file and
    // paste the file's path (so an in-pane CLI can read the image).
    try {
      const items = await navigator.clipboard.read();
      for (const item of items) {
        if (item.types.includes("image/png")) {
          const blob = await item.getType("image/png");
          const bytes = new Uint8Array(await blob.arrayBuffer());
          const path = await api.savePasteImage(Array.from(bytes));
          if (path && this.spawned) {
            // Always quote: the path can contain spaces (e.g. a profile
            // directory like "C:\Users\John Smith\..."), which the
            // receiving shell/CLI would otherwise split into two
            // arguments. Path only — no trailing newline; the user
            // presses Enter.
            void api.writePane(this.id, ENCODER.encode(`"${path}"`));
          }
          return;
        }
      }
    } catch {
      // clipboard.read() unsupported/denied, or save failed — fall through to
      // the text path below.
    }
    // Existing text-paste behaviour, unchanged.
    try {
      const text = await navigator.clipboard.readText();
      if (text && this.spawned) {
        void api.writePane(this.id, ENCODER.encode(text));
      }
    } catch {
      // Clipboard access denied or empty — silent fail.
    }
  }

  private currentDims(): { cols: number; rows: number } {
    const cols = this.term.cols || 80;
    const rows = this.term.rows || 24;
    return { cols, rows };
  }

  /// Debounce scrollback saves to ~2s after output settles, instead of
  /// serializing the whole buffer on every chunk (which would thrash disk
  /// I/O during a fast-scrolling build log or `cat` of a large file).
  private scheduleScrollbackSave(): void {
    if (!this.opts.persistScrollback?.()) return;
    if (this.scrollbackSaveTimer !== undefined) {
      window.clearTimeout(this.scrollbackSaveTimer);
    }
    this.scrollbackSaveTimer = window.setTimeout(() => {
      void api.saveScrollback(this.id, this.serializeAddon.serialize());
    }, 2000);
  }

  /// Tear down this pane. `permanent` distinguishes the two callers:
  ///  - `true`  — the user explicitly closed this pane (WorkspaceManager's
  ///    `closeFocused`) or deleted its workspace (`deleteWorkspace`). The PTY
  ///    is killed AND its saved scrollback is deleted, since there is no
  ///    "next mount" to restore into.
  ///  - `false` (default) — app shutdown. WorkspaceManager never calls
  ///    `dispose()` on this path (see `main.ts`'s `beforeunload` → `flush()`,
  ///    which only persists config); this default exists so `dispose()` is
  ///    safe-by-default (no accidental scrollback deletion) if a future
  ///    caller is added without reading this comment.
  dispose(permanent = false): void {
    this.cleanupLang();
    if (this.statusTimer !== undefined) window.clearInterval(this.statusTimer);
    if (this.scrollbackSaveTimer !== undefined) {
      window.clearTimeout(this.scrollbackSaveTimer);
    }
    window.removeEventListener("beforeunload", this.flushScrollbackOnUnload);
    for (const u of this.unlisteners) u();
    this.unlisteners = [];
    if (this.spawned) {
      void api.killPane(this.id).catch(() => {});
    }
    if (permanent) {
      // Unconditional (not gated on the live toggle): a file may exist from
      // when persistence was previously on, and the backend delete is a
      // no-op if there's nothing to remove, so this can't leave an orphaned
      // scrollback file behind after the user permanently closes the pane.
      void api.deleteScrollback(this.id);
    }
    this.term.dispose();
    this.element.remove();
  }
}
