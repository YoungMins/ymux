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
  private opts: NativeBrowserPaneOptions;
  private cleanupLang: () => void;
  private unlisteners: UnlistenFn[] = [];
  private history: string[] = [];
  private historyIndex = -1;

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
    this.urlInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        const raw = this.urlInput.value.trim();
        if (raw) this.navigate(raw);
      }
    });

    nav.appendChild(this.backBtn);
    nav.appendChild(this.fwdBtn);
    nav.appendChild(this.reloadBtn);
    nav.appendChild(this.urlInput);
    this.element.appendChild(nav);

    // Placeholder — the child window overlays this area
    this.placeholder = document.createElement("div");
    this.placeholder.className = "native-browser-pane__placeholder";
    this.placeholder.style.flex = "1 1 auto";
    this.placeholder.style.minHeight = "0";
    this.placeholder.style.minWidth = "0";
    this.placeholder.style.background = "#1a2230";
    this.element.appendChild(this.placeholder);

    // Track layout changes
    this.resizeObserver = new ResizeObserver(() => this.scheduleReposition());
    this.resizeObserver.observe(this.placeholder);

    this.element.addEventListener("focusin", () => this.opts.onFocus?.());
    this.element.addEventListener("pointerdown", () => this.focus());

    this.cleanupLang = onLangChange(() => {
      this.backBtn.title = t("browser.back");
      this.fwdBtn.title = t("browser.forward");
      this.reloadBtn.title = t("browser.reload");
    });
  }

  async spawn(): Promise<void> {
    if (this.spawned) return;
    const initial = this.url
      ? normalizeUrl(this.url) ?? "https://www.bing.com"
      : "https://www.bing.com";

    const rect = await this.getScreenRect();
    try {
      await api.createWebview(this.id, initial, rect.x, rect.y, rect.width, rect.height);
      this.spawned = true;
      this.urlInput.value = initial;
      this.pushHistory(initial);
    } catch (e) {
      this.placeholder.textContent = `Browser failed: ${e}`;
      throw e;
    }

    // Poll the main window position every ~33ms to keep the child window
    // glued to the placeholder. Tauri's onMoved event only fires AFTER
    // the user releases the title bar drag — that's too late for
    // smooth tracking, so we poll instead. Cheap because getScreenRect
    // and resizeWebview are no-ops if the rect didn't change.
    let lastKey = "";
    this.posPollTimer = window.setInterval(() => {
      if (!this.spawned) return;
      void this.getScreenRect().then((r) => {
        const key = `${r.x},${r.y},${r.width},${r.height}`;
        if (key === lastKey) return;
        lastKey = key;
        void api.resizeWebview(this.id, r.x, r.y, r.width, r.height).catch(() => {});
      });
    }, 33);
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
      console.warn("[NativeBrowser] invalid URL:", raw);
      return;
    }
    console.log("[NativeBrowser] navigate ->", url);
    this.url = url;
    this.urlInput.value = url;
    this.pushHistory(url);
    if (this.spawned) {
      void api.navigateWebview(this.id, url).then(
        () => console.log("[NativeBrowser] navigateWebview returned ok"),
        (e) => console.error("[NativeBrowser] navigateWebview rejected:", e),
      );
    } else {
      console.warn("[NativeBrowser] not spawned yet");
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

  // ── Positioning ─────────────────────────────────────────────────────

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
    const [, scale] = await Promise.all([
      win.outerPosition(),
      win.scaleFactor(),
    ]);
    const domRect = this.placeholder.getBoundingClientRect();

    // outerPosition is in physical pixels. DOM rect is in CSS pixels.
    // On decorated windows, we need to account for the title bar offset.
    // Tauri's outerPosition gives the frame origin; innerPosition gives
    // the content origin. The difference is the title bar + border.
    const innerPos = await win.innerPosition();
    const x = innerPos.x + Math.round(domRect.left * scale);
    const y = innerPos.y + Math.round(domRect.top * scale);
    const width = Math.max(1, Math.round(domRect.width * scale));
    const height = Math.max(1, Math.round(domRect.height * scale));

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
