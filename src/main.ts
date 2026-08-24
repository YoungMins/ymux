// App entry point. Bootstraps the frontend by pulling the initial config +
// detected shells from the Rust backend, then mounts the workspace bar and
// workspace host and wires keyboard shortcuts.

import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { formatDroppedPaths } from "./terminal/dropPaths";
import { api } from "./ipc/bridge";
import { WorkspaceManager, MAX_WORKSPACES } from "./workspace/WorkspaceManager";
import { mountWorkspaceBar } from "./workspace/WorkspaceBar";
import { mountWorkspacePanel, refreshWorkspacePanel } from "./workspace/WorkspacePanel";
import { mountUpdateBanner } from "./update/UpdateBanner";
import { mountStatusBar } from "./statusbar/StatusBar";
import { initLang, t } from "./i18n/i18n";
import { mountCommandPalette, toggle as togglePalette } from "./palette/CommandPalette";
import { builtinCommands } from "./palette/commands";
import { mountNotesOverlay, toggle as toggleNotes } from "./notes/NotesOverlay";
import { askText } from "./ui/Dialog";
import { hasMod, isWorkspaceSwitch } from "./platform";

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

  const host = document.createElement("div");
  host.className = "workspace-host";
  appMain.appendChild(host);

  const manager = new WorkspaceManager(host, bootstrap.config, bootstrap.shells);
  mountWorkspaceBar(appMain, manager, bootstrap.shells);
  // The bar was appended after the host; move it to the top of the column.
  const bar = appMain.querySelector(".workspace-bar");
  if (bar) appMain.insertBefore(bar, host);

  mountWorkspacePanel(panelEl, manager);

  await manager.start();

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
  void listen<{
    key: string;
    code: string;
    ctrl: boolean;
    shift: boolean;
    alt: boolean;
  }>("ymux:forwarded-key", (ev) => {
    const p = ev.payload;
    const synth = new KeyboardEvent("keydown", {
      key: p.key,
      code: p.code,
      ctrlKey: p.ctrl,
      shiftKey: p.shift,
      altKey: p.alt,
      bubbles: true,
      cancelable: true,
    });
    window.dispatchEvent(synth);
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
  // what the user is reaching for.
  document.addEventListener("contextmenu", (ev) => {
    if ((ev.target as HTMLElement | null)?.closest("input, textarea")) return;
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
      const text = formatDroppedPaths(event.payload.paths ?? []);
      if (!text) return;
      const dpr = window.devicePixelRatio || 1;
      manager.typeIntoPaneAt(
        event.payload.position.x / dpr,
        event.payload.position.y / dpr,
        text,
      );
    })
    .catch((e) => console.warn("drag-drop listener failed:", e));

  window.addEventListener("resize", () => manager.refitActive());
  // Coming back from another app is the moment the user is most likely to
  // reach for the wheel first. Refitting also re-syncs each pane's scrollbar
  // with its buffer, so that first notch can't jump to the top of the
  // scrollback (see terminal/viewportSync.ts).
  window.addEventListener("focus", () => manager.refitActive());
  window.addEventListener("beforeunload", () => {
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
