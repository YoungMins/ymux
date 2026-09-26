// App entry point. Bootstraps the frontend by pulling the initial config +
// detected shells from the Rust backend, then mounts the workspace bar and
// workspace host and wires keyboard shortcuts.

// MUST be the first import: refuses to boot inside a frame (security).
import "./bootGuard";
import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { forwardedKeyInit } from "./browser/forwardedKeys";
import { api, onAgentsChanged, onPaneLabels } from "./ipc/bridge";
import { WorkspaceManager, MAX_WORKSPACES } from "./workspace/WorkspaceManager";
import { mountWorkspaceBar } from "./workspace/WorkspaceBar";
import { mountWorkspacePanel, refreshWorkspacePanel } from "./workspace/WorkspacePanel";
import { mountUpdateBanner } from "./update/UpdateBanner";
import { mountStatusBar } from "./statusbar/StatusBar";
import { initLang, onLangChange, t } from "./i18n/i18n";
import { mountCommandPalette, toggle as togglePalette } from "./palette/CommandPalette";
import { builtinCommands } from "./palette/commands";
import { mountNotesOverlay, toggle as toggleNotes } from "./notes/NotesOverlay";
import { askText } from "./ui/Dialog";
import { hasMod, isWorkspaceSwitch } from "./platform";
import { mountFileDock, toggleFileDock } from "./filedock/FileDock";
import { runQuitGuard } from "./tray/quitGuard";

async function main(): Promise<void> {
  initLang();

  const app = document.getElementById("app");
  if (!app) throw new Error("#app mount point missing");

  const bootstrap = await api.loadBootstrap();
  if (bootstrap.shells.length === 0) {
    const warn = document.createElement("div");
    warn.textContent = t("app.noShells");
    warn.style.padding = "20px";
    app.appendChild(warn);
    return;
  }

  // Left workspace panel (full height) + main column (top bar, panes, status).
  const panelEl = document.createElement("div");
  panelEl.className = "workspace-panel-host";
  const appMain = document.createElement("div");
  appMain.className = "app-main";
  app.appendChild(panelEl);
  app.appendChild(appMain);

  // Row under the top bar: the workspace area plus the right-side file dock.
  const body = document.createElement("div");
  body.className = "app-body";
  appMain.appendChild(body);

  const host = document.createElement("div");
  host.className = "workspace-host";
  body.appendChild(host);

  const manager = new WorkspaceManager(host, bootstrap.config, bootstrap.shells);
  mountWorkspaceBar(appMain, manager, bootstrap.shells);
  // The bar was appended after the body row; move it to the top of the
  // column. (`host` is no longer a child of appMain, so anchor on `body`.)
  const bar = appMain.querySelector(".workspace-bar");
  if (bar) appMain.insertBefore(bar, body);

  mountWorkspacePanel(panelEl, manager);

  await manager.start();
  mountFileDock(body, manager);
  // Agent tree: subscribe first, then seed, so no change slips between.
  void onAgentsChanged((s) => manager.applyAgents(s))
    .catch((e) => console.warn("agents:changed listen failed:", e))
    .then(() => api.getAgents())
    .then((s) => manager.applyAgents(s))
    .catch((e) => console.warn("get_agents failed:", e));

  // Tab labels: subscribe first, then seed, so no change slips between.
  void onPaneLabels((labels) => manager.applyPaneLabels(labels))
    .catch((e) => console.warn("panes:labels listen failed:", e))
    .then(() => api.getPaneLabels())
    .then((labels) => manager.applyPaneLabels(labels))
    .catch((e) => console.warn("get_pane_labels failed:", e));

  // Listen for update-available events from the Rust poller. Non-fatal if the
  // listen fails (e.g. capability denied in some harness); app keeps running.
  void mountUpdateBanner(document.body).catch((e) =>
    console.warn("mountUpdateBanner failed:", e),
  );

  // System monitor status bar — sits at the bottom of #app.
  void mountStatusBar(appMain).catch((e) =>
    console.warn("mountStatusBar failed:", e),
  );

  // Command palette (Ctrl+Shift+P)
  mountCommandPalette(document.body, builtinCommands(manager));

  // Notes overlay (Ctrl+Alt+N)
  mountNotesOverlay(document.body);

  // Replay shortcuts that were captured inside a child browser webview
  // (its `initialization_script` forwards them via the `forward_keystroke`
  // command, which re-emits this event). Synthesize a KeyboardEvent so the
  // existing window keydown handler below catches it as if the user had
  // pressed the key inside the main webview.
  //
  // The payload originates from a website, so only the fixed shortcut table
  // in `forwardedKeys.ts` is replayed, keyed by `code` with a derived `key`.
  void listen<unknown>("ymux:forwarded-key", (ev) => {
    const init = forwardedKeyInit(ev.payload);
    if (!init) return;
    window.dispatchEvent(new KeyboardEvent("keydown", init));
  }).catch((e) => console.warn("forwarded-key listen failed:", e));

  // Global keybindings. Tauri's global-shortcut plugin is overkill for
  // window-local bindings — plain DOM events are sufficient inside WebView2.
  window.addEventListener("keydown", (ev) => {
    const key = ev.key;
    // Primary modifier: Cmd on macOS, Ctrl elsewhere. See src/platform.ts.
    const mod = hasMod(ev);

    // Switch workspaces: Ctrl+Alt+1..9 on Windows (Ctrl+Shift+digit is
    // intercepted at the OS level by some apps), plain Cmd+1..9 on macOS.
    // Either way the digit comes from `ev.code`, which is layout-independent
    // ("Digit1"…"Digit9"), so Korean / AZERTY / etc. users who produce a
    // different character on the number row still get the right workspace.
    if (isWorkspaceSwitch(ev)) {
      const id = Number.parseInt(ev.code.slice(-1), 10);
      if (id >= 1 && id <= MAX_WORKSPACES) {
        ev.preventDefault();
        void manager.activate(id).then(() => refreshWorkspacePanel(app));
      }
      return;
    }

    // Ctrl+Alt+N (Cmd+Opt+N on macOS) toggle notes for the active workspace.
    // Layout-independent via ev.code so non-QWERTY users still hit the same
    // physical key.
    if (mod && ev.altKey && !ev.shiftKey && ev.code === "KeyN") {
      ev.preventDefault();
      const wsId = manager.activeIdValue;
      toggleNotes(wsId, manager.getWorkspaceName(wsId));
      refreshWorkspacePanel(app);
      return;
    }

    // Ctrl+Shift+H horizontal split.
    if (mod && ev.shiftKey && (key === "H" || key === "h")) {
      ev.preventDefault();
      void manager.splitFocused("horizontal");
      return;
    }

    // Ctrl+Shift+V vertical split.
    if (mod && ev.shiftKey && (key === "V" || key === "v")) {
      ev.preventDefault();
      void manager.splitFocused("vertical");
      return;
    }

    // Ctrl+Shift+W close focused pane.
    if (mod && ev.shiftKey && (key === "W" || key === "w")) {
      ev.preventDefault();
      void manager.closeFocused();
      return;
    }

    // Ctrl+Shift+T new tab in the focused pane.
    if (mod && ev.shiftKey && (key === "T" || key === "t")) {
      ev.preventDefault();
      void manager.newTabInFocused();
      return;
    }

    // Ctrl+Shift+[ / Ctrl+Shift+] previous / next tab. Matched on `ev.code`
    // for the same layout-independence reason as the digit and font
    // bindings: the character Shift+bracket produces varies by keyboard.
    if (mod && ev.shiftKey && !ev.altKey && ev.code === "BracketLeft") {
      ev.preventDefault();
      manager.stepTabInFocused(-1);
      return;
    }
    if (mod && ev.shiftKey && !ev.altKey && ev.code === "BracketRight") {
      ev.preventDefault();
      manager.stepTabInFocused(1);
      return;
    }

    // Ctrl+Tab cycle. Deliberately Ctrl on macOS too: Cmd+Tab is the OS
    // application switcher and never reaches the webview.
    if (ev.ctrlKey && !ev.shiftKey && key === "Tab") {
      ev.preventDefault();
      manager.cycleFocus(1);
      return;
    }
    if (ev.ctrlKey && ev.shiftKey && key === "Tab") {
      ev.preventDefault();
      manager.cycleFocus(-1);
      return;
    }

    // Ctrl+Shift+Left / Right swap the focused pane with the previous / next
    // pane in depth-first order (wrapping). Arrow keys are layout-independent
    // and unused elsewhere.
    if (mod && ev.shiftKey && !ev.altKey && key === "ArrowLeft") {
      ev.preventDefault();
      manager.swapFocused(-1);
      return;
    }
    if (mod && ev.shiftKey && !ev.altKey && key === "ArrowRight") {
      ev.preventDefault();
      manager.swapFocused(1);
      return;
    }

    // Ctrl+Shift+Z zoom / unzoom focused pane.
    if (mod && ev.shiftKey && (key === "Z" || key === "z")) {
      ev.preventDefault();
      manager.toggleZoomFocused();
      return;
    }

    // Ctrl+F scrollback search on the focused terminal pane.
    if (mod && !ev.shiftKey && !ev.altKey && (key === "F" || key === "f")) {
      ev.preventDefault();
      manager.toggleSearchOnFocused();
      return;
    }

    // Ctrl/Cmd + `+` / `-` / `0` resize the terminal font, the near-universal
    // terminal zoom binding. `ev.code` is layout-independent, and both the
    // main row and the numpad are accepted; Shift is allowed because `+` on
    // most layouts *is* Shift+Equal.
    if (mod && !ev.altKey) {
      if (ev.code === "Equal" || ev.code === "NumpadAdd") {
        ev.preventDefault();
        manager.bumpFontSize(1);
        return;
      }
      if (ev.code === "Minus" || ev.code === "NumpadSubtract") {
        ev.preventDefault();
        manager.bumpFontSize(-1);
        return;
      }
      if (!ev.shiftKey && (ev.code === "Digit0" || ev.code === "Numpad0")) {
        ev.preventDefault();
        manager.resetFontSize();
        return;
      }
    }

    // Ctrl+Shift+E toggle the file dock.
    if (mod && ev.shiftKey && !ev.altKey && (key === "E" || key === "e")) {
      ev.preventDefault();
      toggleFileDock();
      return;
    }

    // Ctrl+Shift+P command palette.
    if (mod && ev.shiftKey && (key === "P" || key === "p")) {
      ev.preventDefault();
      togglePalette();
      return;
    }

    // Ctrl+Shift+R rename focused pane (prompt). Keeping it under Ctrl+Shift
    // so a stray lowercase `r` in a shell still reaches the PTY.
    if (mod && ev.shiftKey && (key === "R" || key === "r")) {
      ev.preventDefault();
      const current = manager.getFocusedTitle() ?? "";
      void askText(t("app.paneTitle"), current).then((next) => {
        if (next !== null) manager.renameFocused(next);
      });
      return;
    }
  });

  // Suppress the webview's own context menu app-wide — it offers browser
  // actions (Reload, Inspect, Back) that mean nothing in a terminal
  // multiplexer. Terminal panes put their own menu up in its place; text
  // inputs keep the native one, where cut/copy/paste on a field is exactly
  // what the user is reaching for — and so does the editor pane's text
  // (CodeMirror's `.cm-content`), for the same reason.
  document.addEventListener("contextmenu", (ev) => {
    if ((ev.target as HTMLElement | null)?.closest("input, textarea, .cm-content")) return;
    ev.preventDefault();
  });

  // Dropping files onto a terminal types their quoted paths, so an in-pane
  // CLI can act on them (same idea as the Ctrl+V image paste). Tauri owns the
  // OS drop — HTML5 drag events never fire for files — and hands us real
  // filesystem paths plus a PHYSICAL-pixel position, which we convert to CSS
  // pixels to hit-test the pane under the cursor. No Enter is sent.
  void getCurrentWebview()
    .onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      // Quoted for the target pane's shell (shellQuote.ts), so a `$` or
      // backtick in a file name is never expanded.
      const dpr = window.devicePixelRatio || 1;
      manager.dropPathsAt(
        event.payload.position.x / dpr,
        event.payload.position.y / dpr,
        event.payload.paths ?? [],
      );
    })
    .catch((e) => console.warn("drag-drop listener failed:", e));

  window.addEventListener("resize", () => manager.refitActive());
  // Coming back from another app is the moment the user is most likely to
  // reach for the wheel first. Refitting also re-syncs each pane's scrollbar
  // with its buffer, so that first notch can't jump to the top of the
  // scrollback (see terminal/viewportSync.ts).
  window.addEventListener("focus", () => manager.refitActive());
  // Close-to-tray (src-tauri/src/tray.rs): the window's close button,
  // Alt+F4, the taskbar's "Close window" only HIDE the window — the backend
  // handles those without asking us, and PTYs keep running. A real quit
  // (tray Quit, macOS Cmd+Q) arrives as `ymux://quit-requested`; the
  // unsaved-changes guard (spec §3.5) runs here and the answer goes back via
  // `answer_quit`, which exits through `final_flush` on a go-ahead.
  //
  // `quit_guard_ready` is sent only once the listener is live: until then
  // the backend quits without asking, so ymux can always be quit.
  let windowClosing = false;
  let quitAsked = false;
  void listen("ymux://quit-requested", async () => {
    // Ack before any prompt: tells the backend this page is alive, so a
    // second Quit while the prompt is up is swallowed, not taken for a
    // hung page (which would exit without asking).
    void api.ackQuit().catch((e) => console.warn("ack_quit failed:", e));
    if (quitAsked) return;
    quitAsked = true;
    let ok = true;
    try {
      ok = await runQuitGuard({
        hasUnsavedEditors: () => manager.hasUnsavedEditors(),
        showWindow: () => api.showMainWindow(),
        confirmCloseAll: () => manager.confirmCloseAll(),
        flushDrafts: () => manager.flushDrafts(),
        flushLayout: () => manager.flush(),
      });
      if (ok) windowClosing = true;
    } finally {
      quitAsked = false;
      // On an accepted go-ahead the page is torn down under this call; if
      // the backend did not take it (cancelled, stale), keep guarding
      // reloads with the `beforeunload` prompt below.
      void api
        .answerQuit(ok)
        .then((exiting) => {
          if (!exiting) windowClosing = false;
        })
        .catch((e) => {
          windowClosing = false;
          console.warn("answer_quit failed:", e);
        });
    }
  })
    .then(() => api.quitGuardReady())
    .catch((e) => console.warn("quit-requested listener failed:", e));

  // Hidden to the tray: nothing is on screen (agent `done` vs `attention`).
  void listen("ymux://main-hidden", () => manager.noteWindowHidden()).catch((e) =>
    console.warn("main-hidden listen failed:", e),
  );
  // Shown again: panes may have resized while hidden, and un-hiding resets
  // the viewport's scrollTop behind xterm's back (rule 14) — refit even if
  // focus never lands in the webview.
  // `main-shown` follows the backend's `set_focus`, which in WebView2 often
  // raises no DOM `focus` — so mark the window seen here, not only there.
  void listen("ymux://main-shown", () => {
    manager.noteWindowShown(true);
    manager.refitActive();
  }).catch((e) => console.warn("main-shown listen failed:", e));
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    manager.noteWindowShown(document.hasFocus());
    manager.refitActive();
  });

  // The tray's menu labels and tooltip are built in English by the backend;
  // push the translations now and on every language change (rule 7).
  const pushTrayLabels = () =>
    void api
      .setTrayLabels(t("tray.open"), t("tray.quit"), t("tray.tooltip"))
      .catch((e) => console.warn("set_tray_labels failed:", e));
  pushTrayLabels();
  onLangChange(pushTrayLabels);

  window.addEventListener("beforeunload", (ev) => {
    // A reload with unsaved editors: let the webview ask. Not on the real
    // quit, which the guard above has already settled.
    if (!windowClosing && manager.hasUnsavedEditors()) {
      ev.preventDefault();
      ev.returnValue = "";
    }
    // Drafts still in their 2 s debounce go to disk now (fire and forget:
    // an unload cannot wait), so a reload or teardown loses no edit.
    void manager.flushDrafts().catch(() => {});
    void manager.flush();
  });
}

main().catch((e) => {
  console.error(e);
  const el = document.getElementById("app");
  if (el) {
    el.textContent = `ymux failed to start: ${(e as Error).message}`;
    el.style.padding = "20px";
  }
});
