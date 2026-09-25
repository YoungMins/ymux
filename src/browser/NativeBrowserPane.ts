// Native browser pane: opens a child WebviewWindow (parented to the main
// window) and keeps it positioned over a placeholder <div>. Bypasses
// X-Frame-Options / CSP restrictions that limit the iframe-based BrowserPane.
//
// The child window tracks the main window's position via onMoved/onResized
// events so it follows when the user drags the main window.

import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { isPopupOpen, registerBlurListener } from "./popupBlur";

export interface NativeBrowserPaneOptions {
  spec: PaneSpec;
  onFocus?: () => void;
  onUrlChange?: (url: string) => void;
}

export class NativeBrowserPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  private url: string;
  private placeholder: HTMLDivElement;
  private urlInput: HTMLInputElement;
  private backBtn: HTMLButtonElement;
  private fwdBtn: HTMLButtonElement;
  private reloadBtn: HTMLButtonElement;
  private resizeObserver: ResizeObserver;
  private spawned = false;
  private repositionRaf: number | null = null;
  private posPollTimer: number | null = null;
  private cachedScale = 1;
  private statusEl: HTMLPreElement;
  private opts: NativeBrowserPaneOptions;
  private cleanupLang: () => void;
  private unlisteners: UnlistenFn[] = [];
  private history: string[] = [];
  private historyIndex = -1;
  private zoomLevel = 1.0;
  private zoomOutBtn!: HTMLButtonElement;
  private zoomInBtn!: HTMLButtonElement;
  private zoomLabel!: HTMLSpanElement;
  private hiddenForPopup = false;
  private unregisterBlur: (() => void) | null = null;

  constructor(opts: NativeBrowserPaneOptions) {
    this.id = opts.spec.id;
    this.opts = opts;
    this.url = opts.spec.url?.trim() || "";

    this.element = document.createElement("div");
    this.element.className = "pane browser-pane";
    this.element.tabIndex = 0;
    this.element.dataset.paneId = this.id;

    const titleEl = document.createElement("div");
    titleEl.className = "pane-title";
    titleEl.textContent = opts.spec.title || t("browser.defaultTitle");
    this.element.appendChild(titleEl);

    // Nav bar
    const nav = document.createElement("div");
    nav.className = "browser-pane__nav";

    this.backBtn = iconBtn("←", t("browser.back"), () => this.goBack());
    this.fwdBtn = iconBtn("→", t("browser.forward"), () => this.goForward());
    this.reloadBtn = iconBtn("⟳", t("browser.reload"), () => this.doReload());

    this.urlInput = document.createElement("input");
    this.urlInput.type = "text";
    this.urlInput.className = "browser-pane__url";
    this.urlInput.placeholder = "https://…";
    this.urlInput.value = this.url;
    this.urlInput.spellcheck = false;
    this.urlInput.disabled = true; // re-enabled after spawn completes
    this.urlInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        const raw = this.urlInput.value.trim();
        if (raw) this.navigate(raw);
      }
    });

    this.zoomOutBtn = iconBtn("−", t("browser.zoomOut"), () => this.changeZoom(-0.1));
    this.zoomLabel = document.createElement("span");
    this.zoomLabel.className = "browser-pane__zoom";
    this.zoomLabel.textContent = "100%";
    this.zoomInBtn = iconBtn("+", t("browser.zoomIn"), () => this.changeZoom(0.1));

    nav.appendChild(this.backBtn);
    nav.appendChild(this.fwdBtn);
    nav.appendChild(this.reloadBtn);
    nav.appendChild(this.urlInput);
    nav.appendChild(this.zoomOutBtn);
    nav.appendChild(this.zoomLabel);
    nav.appendChild(this.zoomInBtn);
    this.element.appendChild(nav);

    // Placeholder — the child window overlays this area
    this.placeholder = document.createElement("div");
    this.placeholder.className = "native-browser-pane__placeholder";
    this.placeholder.style.flex = "1 1 auto";
    this.placeholder.style.minHeight = "0";
    this.placeholder.style.minWidth = "0";
    this.placeholder.style.background = "#1a2230";
    this.placeholder.style.position = "relative";
    this.element.appendChild(this.placeholder);

    // Status overlay — visible debug log for users without DevTools.
    this.statusEl = document.createElement("pre");
    this.statusEl.style.position = "absolute";
    this.statusEl.style.bottom = "4px";
    this.statusEl.style.left = "4px";
    this.statusEl.style.right = "4px";
    this.statusEl.style.margin = "0";
    this.statusEl.style.padding = "4px 6px";
    this.statusEl.style.background = "rgba(11, 15, 20, 0.85)";
    this.statusEl.style.color = "#7fdbca";
    this.statusEl.style.fontSize = "10px";
    this.statusEl.style.lineHeight = "1.3";
    this.statusEl.style.fontFamily = "var(--font-mono)";
    this.statusEl.style.zIndex = "5";
    this.statusEl.style.whiteSpace = "pre-wrap";
    this.statusEl.style.maxHeight = "120px";
    this.statusEl.style.overflow = "auto";
    this.placeholder.appendChild(this.statusEl);

    // Track layout changes
    this.resizeObserver = new ResizeObserver(() => this.scheduleReposition());
    this.resizeObserver.observe(this.placeholder);

    this.element.addEventListener("focusin", () => this.opts.onFocus?.());
    this.element.addEventListener("pointerdown", () => this.focus());

    this.cleanupLang = onLangChange(() => {
      this.backBtn.title = t("browser.back");
      this.fwdBtn.title = t("browser.forward");
      this.reloadBtn.title = t("browser.reload");
      this.zoomOutBtn.title = t("browser.zoomOut");
      this.zoomInBtn.title = t("browser.zoomIn");
    });

    this.setStatus(`constructed id=${this.id.slice(0, 8)}`);
  }

  /// Append a line to the visible status log inside the placeholder.
  /// Last 6 lines kept so the user can see the state history.
  private statusLines: string[] = [];
  private setStatus(msg: string): void {
    this.statusLines.push(msg);
    if (this.statusLines.length > 6) {
      this.statusLines.splice(0, this.statusLines.length - 6);
    }
    this.statusEl.textContent = this.statusLines.join("\n");
  }

  async spawn(): Promise<void> {
    if (this.spawned) {
      this.setStatus("spawn() called again — already spawned");
      return;
    }
    this.setStatus(`spawning id=${this.id.slice(0, 8)}…`);
    const initial = this.url
      ? normalizeUrl(this.url) ?? "https://www.bing.com"
      : "https://www.bing.com";

    const win = getCurrentWindow();
    try {
      this.cachedScale = await win.scaleFactor();
      this.setStatus(`scale=${this.cachedScale}`);
    } catch (e) {
      this.setStatus(`scaleFactor FAILED: ${e}`);
      throw e;
    }

    let rect: { x: number; y: number; width: number; height: number };
    try {
      rect = await this.getScreenRect();
      this.setStatus(`rect=${rect.x},${rect.y} ${rect.width}x${rect.height}`);
    } catch (e) {
      this.setStatus(`getScreenRect FAILED: ${e}`);
      throw e;
    }

    if (rect.width <= 1 || rect.height <= 1) {
      this.setStatus(`waiting for layout…`);
      await new Promise<void>((resolve) => {
        const ro = new ResizeObserver(() => {
          const r = this.placeholder.getBoundingClientRect();
          if (r.width > 1 && r.height > 1) {
            ro.disconnect();
            resolve();
          }
        });
        ro.observe(this.placeholder);
        setTimeout(() => {
          ro.disconnect();
          resolve();
        }, 2000);
      });
      rect = await this.getScreenRect();
      this.setStatus(`rect2=${rect.x},${rect.y} ${rect.width}x${rect.height}`);
    }

    // Eagerly mark spawned and enable the URL bar — the IPC reply may
    // never come back from Rust on Windows (we've seen this with
    // WebviewWindowBuilder), but the underlying webview is still
    // created on the main thread. Don't gate the user's ability to
    // type a URL on the IPC promise that may hang.
    this.spawned = true;
    this.urlInput.value = initial;
    this.urlInput.disabled = false;
    this.pushHistory(initial);
    this.setStatus(`spawned (eager) ${rect.x},${rect.y} ${rect.width}x${rect.height}`);

    api.createWebview(this.id, initial, rect.x, rect.y, rect.width, rect.height).then(
      () => this.setStatus(`createWebview replied OK`),
      (e) => this.setStatus(`createWebview replied ERR: ${e}`),
    );

    this.unregisterBlur = registerBlurListener((blurred) => this.onPopupBlur(blurred));
    if (isPopupOpen()) {
      this.onPopupBlur(true);
    }

    this.startPollLoop();
  }

  private onPopupBlur(blurred: boolean): void {
    if (!this.spawned) return;
    this.hiddenForPopup = blurred;
    void api.setWebviewVisible(this.id, !blurred).catch((e) =>
      this.setStatus(`setWebviewVisible(${!blurred}) ERR: ${e}`),
    );
    if (!blurred) {
      // Force one immediate resize so the child window re-attaches to the
      // current placeholder rect — otherwise the next poll tick (up to 33ms)
      // could briefly show stale position if the layout shifted while hidden.
      void this.reposition();
    }
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
    this.opts.onFocus?.();
  }

  scheduleFit(): void {
    this.scheduleReposition();
  }

  setTitle(title: string | null): void {
    const el = this.element.querySelector(".pane-title");
    if (el) el.textContent = title || t("browser.defaultTitle");
  }

  dispose(): void {
    this.cleanupLang();
    this.resizeObserver.disconnect();
    for (const u of this.unlisteners) u();
    this.unlisteners = [];
    if (this.unregisterBlur) {
      this.unregisterBlur();
      this.unregisterBlur = null;
    }
    if (this.repositionRaf !== null) cancelAnimationFrame(this.repositionRaf);
    if (this.posPollTimer !== null) {
      window.clearInterval(this.posPollTimer);
      this.posPollTimer = null;
    }
    if (this.spawned) {
      this.spawned = false;
      void api.destroyWebview(this.id).catch(() => {});
    }
    this.element.remove();
  }

  // ── Navigation ──────────────────────────────────────────────────────

  private navigate(raw: string): void {
    const url = normalizeUrl(raw);
    if (!url) {
      this.setStatus(`invalid URL: ${raw}`);
      return;
    }
    this.url = url;
    this.urlInput.value = url;
    this.pushHistory(url);
    if (this.spawned) {
      this.setStatus(`navigate -> ${url} (id=${this.id.slice(0, 8)})`);
      void api.navigateWebview(this.id, url).then(
        () => this.setStatus(`navigate OK: ${url}`),
        (e) => this.setStatus(`navigate ERR: ${e}`),
      );
    } else {
      this.setStatus(`ERR: not spawned (id=${this.id.slice(0, 8)} url-was=${this.url})`);
    }
    this.opts.onUrlChange?.(url);
  }

  private goBack(): void {
    if (this.historyIndex <= 0) return;
    this.historyIndex -= 1;
    const url = this.history[this.historyIndex];
    this.url = url;
    this.urlInput.value = url;
    if (this.spawned) void api.navigateWebview(this.id, url).catch(() => {});
    this.opts.onUrlChange?.(url);
  }

  private goForward(): void {
    if (this.historyIndex >= this.history.length - 1) return;
    this.historyIndex += 1;
    const url = this.history[this.historyIndex];
    this.url = url;
    this.urlInput.value = url;
    if (this.spawned) void api.navigateWebview(this.id, url).catch(() => {});
    this.opts.onUrlChange?.(url);
  }

  private doReload(): void {
    if (this.spawned && this.url) {
      void api.navigateWebview(this.id, this.url).catch(() => {});
    }
  }

  private pushHistory(url: string): void {
    this.history = this.history.slice(0, this.historyIndex + 1);
    this.history.push(url);
    this.historyIndex = this.history.length - 1;
  }

  // ── Zoom & UA ────────────────────────────────────────────────────────

  private changeZoom(delta: number): void {
    if (!this.spawned) return;
    this.zoomLevel = Math.max(0.3, Math.min(3.0, Math.round((this.zoomLevel + delta) * 10) / 10));
    this.zoomLabel.textContent = `${Math.round(this.zoomLevel * 100)}%`;
    void api.zoomWebview(this.id, this.zoomLevel).catch((e) => this.setStatus(`zoom ERR: ${e}`));
  }

  // ── Positioning ─────────────────────────────────────────────────────

  private startPollLoop(): void {
    if (this.posPollTimer !== null) window.clearInterval(this.posPollTimer);
    let lastKey = "";
    let pollCount = 0;
    this.posPollTimer = window.setInterval(() => {
      if (!this.spawned || this.hiddenForPopup) return;
      pollCount++;
      void this.getScreenRect().then((r) => {
        const key = `${r.x},${r.y},${r.width},${r.height}`;
        if (key === lastKey) return;
        lastKey = key;
        this.setStatus(`poll #${pollCount} → ${r.x},${r.y} ${r.width}x${r.height}`);
        void api.resizeWebview(this.id, r.x, r.y, r.width, r.height).catch((e) => {
          this.setStatus(`resize ERR: ${e}`);
        });
      });
    }, 33);
  }

  private scheduleReposition(): void {
    if (this.repositionRaf !== null) return;
    this.repositionRaf = requestAnimationFrame(() => {
      this.repositionRaf = null;
      void this.reposition();
    });
  }

  private async reposition(): Promise<void> {
    if (!this.spawned) return;
    const rect = await this.getScreenRect();
    await api.resizeWebview(this.id, rect.x, rect.y, rect.width, rect.height).catch(() => {});
  }

  /// Convert placeholder DOM rect to screen (physical) pixels for the
  /// child WebviewWindow. The child window uses screen coordinates.
  private async getScreenRect(): Promise<{
    x: number;
    y: number;
    width: number;
    height: number;
  }> {
    const win = getCurrentWindow();
    const innerPos = await win.innerPosition();
    const domRect = this.placeholder.getBoundingClientRect();
    const scale = this.cachedScale;

    // innerPosition is in physical pixels (content origin, excluding title
    // bar + borders). DOM rect is in CSS pixels — multiply by scale to get
    // physical, then offset by the window's content origin.
    const x = innerPos.x + Math.round(domRect.left * scale);
    const y = innerPos.y + Math.round(domRect.top * scale);
    const width = Math.max(1, Math.floor(domRect.width * scale) - 1);
    const height = Math.max(1, Math.floor(domRect.height * scale));

    return { x, y, width, height };
  }
}

function iconBtn(icon: string, title: string, onClick: () => void): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "browser-pane__btn";
  btn.textContent = icon;
  btn.title = title;
  btn.addEventListener("click", (ev) => {
    ev.preventDefault();
    onClick();
  });
  return btn;
}

function normalizeUrl(input: string): string | null {
  let candidate = input.trim();
  if (!candidate) return null;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(candidate)) {
    candidate = `https://${candidate}`;
  }
  try {
    const url = new URL(candidate);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url.toString();
  } catch {
    return null;
  }
}
