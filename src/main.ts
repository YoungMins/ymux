// App entry point. Bootstraps the frontend by pulling the initial config +
// detected shells from the Rust backend, then mounts the workspace bar and
// workspace host and wires keyboard shortcuts.

import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { api } from "./ipc/bridge";
import { WorkspaceManager, MAX_WORKSPACES } from "./workspace/WorkspaceManager";
import { mountWorkspaceBar, refreshWorkspaceBar } from "./workspace/WorkspaceBar";
import { mountUpdateBanner } from "./update/UpdateBanner";
import { mountStatusBar } from "./statusbar/StatusBar";
import { initLang, t } from "./i18n/i18n";
import { mountCommandPalette, toggle as togglePalette } from "./palette/CommandPalette";
import { builtinCommands } from "./palette/commands";
import { mountNotesOverlay, toggle as toggleNotes } from "./notes/NotesOverlay";
import { promptWithBlur } from "./browser/popupBlur";

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

  const host = document.createElement("div");
  host.className = "workspace-host";
  app.appendChild(host);

  const manager = new WorkspaceManager(host, bootstrap.config, bootstrap.shells);
  mountWorkspaceBar(app, manager, bootstrap.shells);
  // The bar was appended after the host; move it to the top.
  const bar = app.querySelector(".workspace-bar");
  if (bar) app.insertBefore(bar, host);

  await manager.start();

  // Listen for update-available events from the Rust poller. Non-fatal if the
  // listen fails (e.g. capability denied in some harness); app keeps running.
  void mountUpdateBanner(document.body).catch((e) =>
    console.warn("mountUpdateBanner failed:", e),
  );

  // System monitor status bar — sits at the bottom of #app.
  void mountStatusBar(app).catch((e) =>
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

    // Ctrl+Alt+1..9 switch workspaces. We use Ctrl+Alt instead of Ctrl+Shift
    // (which some Windows apps intercept at the OS level) and check both the
    // key value and the digit codes so Korean / AZERTY / etc. users who
    // produce a different character on the number row still get the correct
    // workspace. `ev.code` is layout-independent ("Digit1"…"Digit9").
    if (ev.ctrlKey && ev.altKey && !ev.shiftKey && /^Digit[1-9]$/.test(ev.code)) {
      const id = Number.parseInt(ev.code.slice(-1), 10);
      if (id >= 1 && id <= MAX_WORKSPACES) {
        ev.preventDefault();
        void manager.activate(id).then(() => refreshWorkspaceBar(app));
      }
      return;
    }

    // Ctrl+Alt+N toggle notes for the active workspace. Layout-independent
    // via ev.code so non-QWERTY users still hit the same physical key.
    if (ev.ctrlKey && ev.altKey && !ev.shiftKey && ev.code === "KeyN") {
      ev.preventDefault();
      const wsId = manager.activeIdValue;
      toggleNotes(wsId, manager.getWorkspaceName(wsId));
      refreshWorkspaceBar(app);
      return;
    }

    // Ctrl+Shift+H horizontal split.
    if (ev.ctrlKey && ev.shiftKey && (key === "H" || key === "h")) {
      ev.preventDefault();
      void manager.splitFocused("horizontal");
      return;
    }

    // Ctrl+Shift+V vertical split.
    if (ev.ctrlKey && ev.shiftKey && (key === "V" || key === "v")) {
      ev.preventDefault();
      void manager.splitFocused("vertical");
      return;
    }

    // Ctrl+Shift+W close focused pane.
    if (ev.ctrlKey && ev.shiftKey && (key === "W" || key === "w")) {
      ev.preventDefault();
      void manager.closeFocused();
      return;
    }

    // Ctrl+Tab cycle.
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
    if (ev.ctrlKey && ev.shiftKey && !ev.altKey && key === "ArrowLeft") {
      ev.preventDefault();
      manager.swapFocused(-1);
      return;
    }
    if (ev.ctrlKey && ev.shiftKey && !ev.altKey && key === "ArrowRight") {
      ev.preventDefault();
      manager.swapFocused(1);
      return;
    }

    // Ctrl+Shift+Z zoom / unzoom focused pane.
    if (ev.ctrlKey && ev.shiftKey && (key === "Z" || key === "z")) {
      ev.preventDefault();
      manager.toggleZoomFocused();
      return;
    }

    // Ctrl+F scrollback search on the focused terminal pane.
    if (ev.ctrlKey && !ev.shiftKey && !ev.altKey && (key === "F" || key === "f")) {
      ev.preventDefault();
      manager.toggleSearchOnFocused();
      return;
    }

    // Ctrl+Shift+P command palette.
    if (ev.ctrlKey && ev.shiftKey && (key === "P" || key === "p")) {
      ev.preventDefault();
      togglePalette();
      return;
    }

    // Ctrl+Shift+R rename focused pane (prompt). Keeping it under Ctrl+Shift
    // so a stray lowercase `r` in a shell still reaches the PTY.
    if (ev.ctrlKey && ev.shiftKey && (key === "R" || key === "r")) {
      ev.preventDefault();
      const current = manager.getFocusedTitle() ?? "";
      const next = promptWithBlur(t("app.paneTitle"), current);
      if (next !== null) manager.renameFocused(next);
      return;
    }
  });

  window.addEventListener("resize", () => manager.refitActive());
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
