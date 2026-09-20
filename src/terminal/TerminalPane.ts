// Wraps a single xterm.js Terminal + its addons and bridges stdin/stdout with
// the Rust PTY session via `api.spawnPane`, `api.writePane`, `onPaneData`.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon } from "@xterm/addon-search";
import { CanvasAddon } from "@xterm/addon-canvas";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import "@xterm/xterm/css/xterm.css";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { HotKeyDef, PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api, describeError, onPaneData, onPaneExit } from "../ipc/bridge";
import { HotKeyBar } from "./HotKeyBar";
import { t, onLangChange } from "../i18n/i18n";
import { PaneStatusMachine, type PaneStatus } from "./paneStatus";
import {
  restoreScrollGuard,
  restoreRevealLines,
  shouldDeferRestoreReveal,
} from "./restoreGuard";
import { resyncNudge } from "./viewportSync";
import { anchorTransform, bufferAnchorOffset } from "./bottomAnchor";
import { shouldSaveScrollback, isUserActivity } from "./scrollbackPersist";
import { hasMod, isWorkspaceSwitch } from "../platform";
import { ImeBridge, isCompositionKey } from "./ime";
import { decideImagePaste, preparePaste } from "./paste";
import { DEFAULT_FONT_SIZE } from "../workspace/fontSize";

export interface TerminalPaneOptions {
  spec: PaneSpec;
  /// Terminal font size in CSS pixels. Global, not per-pane — the owner reads
  /// it from config and passes the same value to every pane it builds.
  fontSize?: number;
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
  /// Whether this pane is on screen right now — the app window is focused and
  /// this pane's workspace is the visible one. Only the owner knows both, so
  /// it supplies the answer; the pane's own `isFocused` flag can't stand in,
  /// because nothing lowers it when the user switches workspaces or alt-tabs
  /// away, and because split panes are all visible at once whether or not
  /// they hold the keyboard focus.
  isVisible?: () => boolean;
  /// Right-click inside this pane. The default webview menu is already
  /// suppressed; the owner supplies the replacement because the useful
  /// entries (split, launch a tool) need workspace-level context.
  onContextMenu?: (ev: MouseEvent) => void;
  /// Fired whenever this pane's derived status (idle/running/done/attention)
  /// changes, so the owner can render a per-pane status indicator.
  onStatusChange?: (status: PaneStatus) => void;
  /// Returns whether scrollback persistence is currently enabled (read live
  /// from WorkspaceManager's toggle rather than snapshotted at pane-creation
  /// time, so flipping the setting takes effect immediately).
  persistScrollback?: () => boolean;
  /// Whether the bottom-anchored prompt is enabled (read live, like
  /// `persistScrollback`). Absent means off, so a standalone pane renders
  /// exactly like plain xterm.
  bottomAnchor?: () => boolean;
  /// Run this program directly instead of the spec's shell. Its exit is the
  /// pane's exit (`onExit`). The file dock uses it to host `ydir`.
  argv?: string[];
  /// Render this pane without its own title row and hotkey bar. Tabs use it:
  /// a `PaneGroup` draws one shared title + hotkey bar above the strip and
  /// binds them to the active tab (spec §4). Absent or `true` leaves every
  /// standalone pane exactly as it is today.
  ownChrome?: boolean;
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
  private hotkeyBar: HotKeyBar | null = null;
  private titleEl: HTMLElement | null = null;
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
  /// xterm's scrollable viewport `<div>`, resolved lazily after `term.open()`
  /// created it. Cached because `resyncViewportScroll` runs per animation
  /// frame while a gutter is being dragged.
  private viewportEl: HTMLElement | null = null;
  /// xterm's `.xterm-screen`, the element the bottom anchor translates.
  /// Resolved once after `term.open()`. It holds the canvases, the helper
  /// textarea / IME preview and the decoration layer, but not the
  /// `.xterm-viewport` scroll element, which must stay put so the scrollbar
  /// and wheel keep working. See `bottomAnchor.ts`.
  private screenEl: HTMLElement | null = null;
  /// rAF coalescing the non-render triggers of `applyBottomAnchor`.
  private pendingAnchorRaf = 0;
  /// Last `transform` written to `screenEl`, so an unchanged offset costs no
  /// style write on every rendered frame.
  private appliedAnchor = "";
  /// Lines to scroll up once the shell has painted its first output, to bring
  /// restored scrollback back into view. 0 = nothing to reveal.
  private pendingRestoreReveal = 0;
  /// Set when a restore landed in a pane with no layout box (a tab spawned
  /// while hidden): the reveal amount depends on the row count, which is
  /// still xterm's 80×24 default at that point, so it is recomputed and
  /// applied at the first fit that can measure the pane. See
  /// `shouldDeferRestoreReveal`.
  private restoreRevealDeferred = false;
  /// Whether the user has typed in this pane during this app run. Gates
  /// scrollback persistence so an idle, restored-but-untouched pane never
  /// re-saves and can't compound its own history across restarts.
  private hadUserActivity = false;
  private cleanupLang: () => void = () => {};
  private statusMachine = new PaneStatusMachine((s) => this.opts.onStatusChange?.(s));
  private isFocused = false;
  private statusTimer: number | undefined;
  private scrollbackSaveTimer: number | undefined;
  /// Owns IME input instead of xterm, which drops the in-place syllable
  /// revisions a Hangul IME is built out of. Created in `open()`, once the
  /// helper textarea exists. See `ime.ts`.
  private ime: ImeBridge | undefined;
  private flushScrollbackOnUnload = (): void => {
    if (
      shouldSaveScrollback({
        persistEnabled: this.opts.persistScrollback?.() ?? false,
        hadUserActivity: this.hadUserActivity,
      })
    ) {
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

    // Right-click opens ymux's own menu. Text inputs living inside the pane
    // (the search bar) keep the native one, where cut/copy/paste on an input
    // is what the user expects.
    this.element.addEventListener("contextmenu", (ev) => {
      if ((ev.target as HTMLElement | null)?.closest("input, textarea")) return;
      ev.preventDefault();
      this.opts.onContextMenu?.(ev);
    });

    // xterm mounts into a child element (not `this.element` directly) so the
    // HotKeyBar sibling doesn't get clobbered when xterm rearranges its
    // internal DOM subtree.
    this.termHost = document.createElement("div");
    this.termHost.className = "pane__term";
    this.element.appendChild(this.termHost);

    if (opts.ownChrome === false) {
      this.element.classList.add("pane--tab");
    } else {
      this.buildChrome();
    }

    const bgColor = opts.spec.bg_color || "#0b0f14";
    this.term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily:
        "Cascadia Code, Consolas, 'Courier New', ui-monospace, monospace",
      fontSize: opts.fontSize ?? DEFAULT_FONT_SIZE,
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
    // Unicode 11 widths. xterm ships a Unicode 6 table by default, which
    // predates emoji: it calls them one cell wide, the font draws two, and the
    // overhang smears into the neighbouring cell and survives the redraw. The
    // provider has to be registered before it can be selected, and the version
    // is a string, not a number.
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = "11";

    // Block xterm.js from consuming ymux-level hotkeys. Without this, Ctrl+F
    // etc. get translated into control bytes (Ctrl+F → 0x06) and written to
    // the PTY, never reaching our window keydown listener. Returning `false`
    // tells xterm to skip its own handling; the DOM event still bubbles up.
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown") return true;
      // Keystrokes the IME owns are handled entirely by `ImeBridge`, off the
      // helper textarea. Returning `false` here keeps xterm's own
      // `CompositionHelper.keydown` out of it; the same call also tells the
      // bridge when a non-IME key ends a run, so it stops diffing against a
      // buffer the shell has moved on from. See `ime.ts`.
      if (this.ime?.handleKeyDown(ev) ?? isCompositionKey(ev)) return false;
      // Everything ymux claims hangs off the primary modifier — Cmd on
      // macOS, Ctrl elsewhere. Keying off `hasMod` rather than `ev.ctrlKey`
      // is what leaves Ctrl+F / Ctrl+V free to reach the shell on macOS,
      // where they mean forward-char and literal-next.
      if (hasMod(ev) && !ev.altKey) {
        const k = ev.key.toLowerCase();
        // Ctrl/Cmd+V → paste clipboard text into the PTY instead of
        // letting xterm send the raw 0x16 byte.
        if (!ev.shiftKey && k === "v") {
          ev.preventDefault();
          void this.pasteClipboard();
          return false;
        }
        if (!ev.shiftKey && k === "f") return false;
        if (ev.shiftKey && (k === "h" || k === "v" || k === "w" || k === "z" || k === "r" || k === "p" || k === "e" || k === "t")) return false;
        // Ctrl/Cmd+Shift+[ / ] → previous / next tab, handled at window level.
        // On `code`, because Shift+bracket is layout-dependent.
        if (ev.shiftKey && (ev.code === "BracketLeft" || ev.code === "BracketRight")) return false;
        // Ctrl/Cmd+Shift+Left/Right → swap pane position (window level).
        if (ev.shiftKey && (k === "arrowleft" || k === "arrowright")) return false;
        // Font zoom. Matched on `code` for the same layout-independence
        // reason as the window-level handler, and deliberately not under a
        // `!ev.shiftKey` guard because `+` *is* Shift+Equal on most layouts.
        //
        // It has to stay inside this modifier block: at the top level it
        // swallowed a bare `-`, `=`, `0` and `+` in every pane, on every
        // platform.
        if (
          ev.code === "Equal" ||
          ev.code === "Minus" ||
          ev.code === "NumpadAdd" ||
          ev.code === "NumpadSubtract" ||
          ev.code === "Digit0" ||
          ev.code === "Numpad0"
        ) {
          return false;
        }
      }
      // Pane cycling stays on Ctrl+Tab on every platform (Cmd+Tab belongs to
      // the macOS app switcher), so it is checked outside the block above.
      if (ev.ctrlKey && !ev.altKey && ev.key.toLowerCase() === "tab") return false;
      if (isWorkspaceSwitch(ev)) return false;
      return true;
    });
    // Custom link handler: Ctrl+click (Cmd+click on macOS) opens the URL in
    // the system browser via the Rust backend instead of the default
    // WebLinksAddon behaviour (which tries `window.open` — unreliable inside
    // WebView2).
    this.term.loadAddon(
      new WebLinksAddon((ev, uri) => {
        if (hasMod(ev)) {
          ev.preventDefault();
          void api.openUrl(uri).catch((e) =>
            console.warn("openUrl failed:", e),
          );
        }
      }),
    );
    this.term.open(this.termHost);
    // IME input is ours now — see `ime.ts` for why xterm cannot keep a Hangul
    // syllable intact on macOS. Installed here because the helper textarea and
    // the `.composition-view` div only exist after `open()`, and rooted at
    // `this.term.element` because it is their common ancestor, which is what
    // lets a capture-phase listener run ahead of xterm's own.
    const xtermRoot = this.term.element;
    const helperTextarea = this.term.textarea;
    if (xtermRoot && helperTextarea) {
      this.ime = new ImeBridge({
        root: xtermRoot,
        textarea: helperTextarea,
        view: xtermRoot.querySelector<HTMLElement>(".composition-view"),
        font: () => ({
          family: this.term.options.fontFamily ?? "monospace",
          size: this.term.options.fontSize ?? DEFAULT_FONT_SIZE,
        }),
        // Route through xterm rather than straight to `api.writePane`, so IME
        // text takes the exact same path as a typed character: `onData` marks
        // user activity, feeds the status machine and writes the PTY. `true`
        // marks it as user input.
        send: (data) => this.term.input(data, true),
      });
      this.ime.install();
    }
    // Serialize addon: snapshots the buffer (text + escape sequences) so it
    // can be replayed on next mount when scrollback persistence is enabled.
    this.term.loadAddon(this.serializeAddon);

    // Bottom-anchored prompt. `onRender` fires from inside xterm's own render
    // frame, so applying there lands in the same paint as the new content.
    // Deferring it would show one frame of fresh output below the clip edge.
    // The other triggers share one rAF.
    this.screenEl =
      this.term.element?.querySelector<HTMLElement>(".xterm-screen") ?? null;
    this.term.onRender(() => this.applyBottomAnchor());
    this.term.onResize(() => this.scheduleBottomAnchor());
    this.term.onScroll(() => this.scheduleBottomAnchor());
    this.term.buffer.onBufferChange(() => this.scheduleBottomAnchor());

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
      () => this.statusMachine.tick(Date.now(), this.visible()),
      1000,
    );

    // Bell (BEL 0x07) → attention signal with no message.
    this.term.onBell(() => {
      opts.onAttention?.(null);
      this.statusMachine.onAttention(this.visible(), Date.now());
    });
    // OSC 9 → attention with the payload text as the message. Only the
    // iTerm2-style plain-text form (`OSC 9 ; <message>`) is a completion
    // notification. Windows Terminal / ConEmu reuse OSC 9 for progress
    // (`9;4;…`), cwd (`9;9;…`), etc., whose payload starts with "<digit>;" —
    // swallow those without alerting so long-running tools don't spam beeps.
    this.term.parser.registerOscHandler(9, (data) => {
      if (!/^\d+;/.test(data)) {
        opts.onAttention?.(data || null);
        this.statusMachine.onAttention(this.visible(), Date.now());
      }
      return true;
    });

    this.term.onData((data) => {
      // Real typing (not terminal auto-responses like focus/cursor reports,
      // which also arrive here) marks the pane as worked-in this session. That
      // gates scrollback persistence (see scheduleScrollbackSave) so an idle,
      // untouched terminal never re-saves and can't pile up its own restored
      // history across open/close cycles.
      if (!this.hadUserActivity && isUserActivity(data)) {
        this.hadUserActivity = true;
        // TEMPORARY diagnostic — remove once idle no-save is confirmed. Shows
        // exactly what first tripped the activity flag, so if an idle pane
        // still saves we can see whether typing or a stray sequence did it.
        console.log(
          `[scrollback-diag] activity set by onData: ${JSON.stringify(data)}`,
        );
      }
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
    if (this.titleEl && !this.spec.title && !this.spec.shell) {
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
          // shell has painted (see the data listener below) — or, for a tab
          // spawned while hidden, at the first fit that can measure the pane,
          // since `this.term.rows` is still the 80×24 default here.
          if (shouldDeferRestoreReveal(true, this.measurable())) {
            this.restoreRevealDeferred = true;
          } else {
            this.pendingRestoreReveal = restoreRevealLines(this.term.rows);
          }
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
      this.term.write(bytes, () => this.revealRestored());
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
        argv: this.opts.argv,
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
          // Same reason as the HotKeyBar's onSubmit: this write bypasses
          // xterm's onData, so tell the status machine a command started or
          // the pane would sit at `idle` while the command runs.
          this.statusMachine.onSubmit(Date.now());
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

  /// The agent registry reports this pane's agent is waiting on the user.
  markWaiting(): void {
    this.statusMachine.onWaiting();
  }

  /// A command was submitted to this pane by something that bypasses xterm's
  /// `onData` — the group's shared HotKey bar, which writes straight to
  /// `writePane`. Without it the pane would sit at `idle` while the command
  /// runs, the same gap `startup_cmd` and the per-pane bar already close.
  noteSubmit(): void {
    this.statusMachine.onSubmit(Date.now());
  }

  get status(): PaneStatus {
    return this.statusMachine.status;
  }

  /// "Am I on screen?" — the owner's answer when it supplies one, otherwise
  /// this pane's own focus flag (standalone/test use).
  private visible(): boolean {
    return this.opts.isVisible?.() ?? this.isFocused;
  }

  /// Whether the terminal currently has selected text, for enabling "Copy".
  hasSelection(): boolean {
    return this.term.hasSelection();
  }

  /// Copy the current selection to the clipboard. No-op without a selection.
  async copySelection(): Promise<void> {
    const text = this.term.getSelection();
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      console.warn("clipboard write failed:", e);
    }
  }

  /// Paste the clipboard into the PTY — the same path as Ctrl+V, images
  /// included.
  async paste(): Promise<void> {
    await this.pasteClipboard();
  }

  /// Submit `cmd` to the shell as if the user had typed it and pressed Enter.
  /// Tells the status machine first: this write bypasses xterm's `onData`, so
  /// without it the pane would stay `idle` while the command runs (the same
  /// gap the HotKey bar and `startup_cmd` had).
  runCommand(cmd: string): void {
    if (!this.spawned) return;
    this.statusMachine.onSubmit(Date.now());
    void api.writePane(this.id, ENCODER.encode(`${cmd}\r`));
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
  /// Change the terminal font size and re-fit. Re-fitting is the load-bearing
  /// half: xterm keeps its row/column count when the glyph size changes, so
  /// without it the terminal keeps the old grid and the PTY is told the wrong
  /// size — long lines wrap in the wrong place until the window is resized.
  setFontSize(px: number): void {
    this.term.options.fontSize = px;
    this.scheduleFit();
  }

  setBgColor(color: string | null): void {
    const bg = color || "#0b0f14";
    this.spec = { ...this.spec, bg_color: color ?? "" };
    this.term.options.theme = { ...this.term.options.theme, background: bg };
    this.element.style.background = bg;
  }

  /// Write a hotkey list edited elsewhere back into this pane. The shared
  /// `HotKeyBar` of a `PaneGroup` edits the active tab's list, and the tab's
  /// own `PaneSpec` copy has to follow — `buildChrome()` seeds its bar from
  /// `this.spec` when the group unwraps and the pane gets its chrome back,
  /// so without this the pane came back with its pre-grouping hotkeys.
  /// Mirrors `setBgColor`. The `?.` is the grouped case: a tab has no bar of
  /// its own, which is exactly when this is called.
  setHotKeys(hotkeys: HotKeyDef[]): void {
    this.spec = { ...this.spec, hotkeys };
    this.hotkeyBar?.bind(this.id, hotkeys, this.spec.bg_color || null);
  }

  setTitle(title: string | null): void {
    this.spec = { ...this.spec, title };
    if (this.titleEl) {
      this.titleEl.textContent = title || this.spec.shell || t("terminal.defaultTitle");
    }
  }

  /// Build this pane's own title row and hotkey bar, in front of the terminal
  /// host. Split out of the constructor because a pane can gain and lose its
  /// chrome at runtime — see `setOwnChrome`.
  private buildChrome(): void {
    // Title label shown above the hotkey bar. Falls back to the shell name
    // when no user title has been set (via `Ctrl+Shift+R`).
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.titleEl.textContent =
      this.spec.title || this.spec.shell || t("terminal.defaultTitle");
    this.element.insertBefore(this.titleEl, this.termHost);

    // Mount the HotKeyBar above xterm. An empty hotkey list still renders a
    // visible ⚙ button so the user can discover the feature.
    this.hotkeyBar = new HotKeyBar({
      paneId: this.id,
      initial: this.spec.hotkeys ?? [],
      initialBgColor: this.spec.bg_color ?? null,
      onSubmit: () => this.statusMachine.onSubmit(Date.now()),
      onChange: (next) => {
        this.spec = { ...this.spec, hotkeys: next };
        this.opts.onHotKeysChange?.(next);
      },
      onBgColorChange: (color) => {
        this.setBgColor(color);
        this.opts.onBgColorChange?.(color);
      },
    });
    this.element.insertBefore(this.hotkeyBar.element, this.termHost);
  }

  /// Add or drop this pane's own title row and hotkey bar. A pane loses them
  /// when it is wrapped in a tab group (the `PaneGroup` draws one shared set
  /// for every tab) and gets them back when that group unwraps to a plain
  /// pane — and the PTY survives both, so this has to be a live toggle rather
  /// than a constructor-only option. Idempotent; `WorkspaceManager` calls it
  /// for every pane on every render.
  setOwnChrome(enabled: boolean): void {
    if (enabled === (this.titleEl !== null)) return;
    if (enabled) {
      this.element.classList.remove("pane--tab");
      this.buildChrome();
      return;
    }
    this.element.classList.add("pane--tab");
    this.titleEl?.remove();
    this.titleEl = null;
    this.hotkeyBar?.dispose();
    this.hotkeyBar?.element.remove();
    this.hotkeyBar = null;
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
        // Every caller of this method runs right after the pane became
        // visible again or was re-parented by a layout rebuild — both of
        // which reset the DOM scrollbar behind xterm's back.
        this.resyncViewportScroll();
        this.settleDeferredRestore();
      } catch {
        // fit throws when the element has zero size; ignore.
      }
    });
  }

  /// Does this pane have a layout box right now? A hidden tab
  /// (`.pane--tab-hidden`, `display: none`) has none, so `FitAddon` cannot
  /// measure it and xterm stays at its 80×24 default.
  private measurable(): boolean {
    return this.termHost.clientHeight > 0 && this.termHost.clientWidth > 0;
  }

  /// Scroll restored scrollback back into view, once. Called from the PTY
  /// data callback (so it lands after the shell's opening `\x1b[2J`) and from
  /// `settleDeferredRestore`.
  private revealRestored(): void {
    if (this.pendingRestoreReveal <= 0) return;
    this.term.scrollLines(-this.pendingRestoreReveal);
    this.pendingRestoreReveal = 0;
  }

  /// A restore that had to wait for a real layout box: the pane has just been
  /// fitted, so the row count is finally the one the user sees. Compute the
  /// reveal from it and apply it now — the shell's startup burst is long
  /// past by the time a hidden tab is shown, so waiting for more PTY data
  /// would leave the history parked out of sight indefinitely.
  private settleDeferredRestore(): void {
    if (!this.restoreRevealDeferred || !this.measurable()) return;
    this.restoreRevealDeferred = false;
    this.pendingRestoreReveal = restoreRevealLines(this.term.rows);
    this.revealRestored();
  }

  /// Put xterm's DOM scrollbar back in step with the buffer after a layout
  /// rebuild or a `display: none` round trip reset it to 0. Without this the
  /// next wheel notch is measured from the wrong origin and jumps the view to
  /// the top of the scrollback. See `viewportSync.ts` for the full mechanism.
  resyncViewportScroll(): void {
    const buf = this.term.buffer.active;
    this.viewportEl ??=
      this.termHost.querySelector<HTMLElement>(".xterm-viewport");
    const steps = resyncNudge(
      buf.viewportY,
      buf.baseY,
      this.viewportEl?.scrollTop ?? null,
    );
    for (const step of steps) this.term.scrollLines(step);
  }

  /// Recompute and apply the bottom anchor now. The owner calls this when the
  /// setting flips; turning it off clears the transform.
  refreshBottomAnchor(): void {
    this.applyBottomAnchor();
  }

  private scheduleBottomAnchor(): void {
    if (this.pendingAnchorRaf) return;
    this.pendingAnchorRaf = requestAnimationFrame(() => {
      this.pendingAnchorRaf = 0;
      this.applyBottomAnchor();
    });
  }

  /// Translate `.xterm-screen` down so short content sits on the pane's last
  /// row. Purely visual: pointer mapping follows because xterm measures
  /// `screenElement.getBoundingClientRect()`, which includes the transform.
  private applyBottomAnchor(): void {
    if (this.pendingAnchorRaf) {
      cancelAnimationFrame(this.pendingAnchorRaf);
      this.pendingAnchorRaf = 0;
    }
    const screen = this.screenEl;
    if (!screen) return;
    const rows = this.term.rows;
    const offset = this.opts.bottomAnchor?.()
      ? bufferAnchorOffset(this.term.buffer.active, rows)
      : 0;
    const transform = anchorTransform(offset, parseFloat(screen.style.height), rows);
    if (transform === this.appliedAnchor) return;
    this.appliedAnchor = transform;
    screen.style.transform = transform;
  }

  private async pasteClipboard(): Promise<void> {
    // Image first: if the clipboard holds an image, it is now a PNG on disk
    // and what gets typed is that file's path, so an in-pane CLI can read it.
    //
    // The clipboard read happens in Rust (`paste_clipboard_image`), not via
    // `navigator.clipboard.read()`. WebView2's async clipboard image read
    // returned blobs whose `arrayBuffer()` was empty, which the backend
    // faithfully wrote to disk as 0-byte PNGs whose paths were then typed into
    // the shell. The webview branch is gone rather than kept as a fallback:
    // the failure was silent and produced a plausible-looking path, so falling
    // back to it would just reinstate the bug in the cases that matter.
    //
    // Image *before* text is deliberate and unchanged: a clipboard carrying
    // both (copying a cell range out of Excel, say) pastes the image path.
    try {
      const decision = decideImagePaste(await api.pasteClipboardImage());
      if (decision.kind === "image") {
        if (this.spawned) {
          void api.writePane(this.id, ENCODER.encode(decision.write));
        }
        return;
      }
    } catch (e) {
      // An image *was* on the clipboard but could not be saved. Say so where
      // the user is looking, and stop: silently pasting the clipboard's text
      // instead would be worse than nothing, and typing a path to a file that
      // isn't there is the bug this replaced.
      this.term.writeln(
        `\x1b[31mcould not paste the clipboard image: ${describeError(e)}\x1b[0m`,
      );
      return;
    }
    // Text. Framed and sanitized by `preparePaste` — bracketed when the
    // foreground app asked for it (xterm tracks DECSET 2004 for us in
    // `term.modes`), ESC-defanged always, and chunked when huge. Awaited in
    // order: a `void` loop would not guarantee the IPC sees the chunks in
    // sequence, and a reordered chunk is a scrambled paste.
    try {
      const text = await navigator.clipboard.readText();
      if (!text || !this.spawned) return;
      const chunks = preparePaste(text, {
        bracketed: this.term.modes.bracketedPasteMode,
      });
      for (const chunk of chunks) {
        await api.writePane(this.id, ENCODER.encode(chunk));
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
    if (
      !shouldSaveScrollback({
        persistEnabled: this.opts.persistScrollback?.() ?? false,
        hadUserActivity: this.hadUserActivity,
      })
    ) {
      return;
    }
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
    this.hotkeyBar?.dispose();
    if (this.statusTimer !== undefined) window.clearInterval(this.statusTimer);
    if (this.scrollbackSaveTimer !== undefined) {
      window.clearTimeout(this.scrollbackSaveTimer);
    }
    window.removeEventListener("beforeunload", this.flushScrollbackOnUnload);
    if (this.pendingAnchorRaf) cancelAnimationFrame(this.pendingAnchorRaf);
    for (const u of this.unlisteners) u();
    this.unlisteners = [];
    this.ime?.dispose();
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
