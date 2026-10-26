// Embedded browser pane: uses Tauri 2's Window::add_child to embed a webview
// as a true child of the main window. Unlike NativeBrowserPane (separate
// WebviewWindow overlay), this approach:
//   - Bypasses X-Frame-Options / CSP (no iframe restrictions)
//   - Requires no polling — ResizeObserver drives bounds updates
//   - Preserves correct z-order, focus, and virtual-desktop behaviour

import type { PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { isPopupOpen, registerBlurListener } from "./popupBlur";

export interface EmbeddedBrowserPaneOptions {
  spec: PaneSpec;
  onFocus?: () => void;
  onUrlChange?: (url: string) => void;
}

export class EmbeddedBrowserPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  private url: string;
  private placeholder: HTMLDivElement;
  private urlInput: HTMLInputElement;
  private backBtn: HTMLButtonElement;
  private fwdBtn: HTMLButtonElement;
  private reloadBtn: HTMLButtonElement;
  private resizeObserver: ResizeObserver;
  private fitRaf: number | null = null;
  private spawned = false;
  private opts: EmbeddedBrowserPaneOptions;
  private cleanupLang: () => void;
  private history: string[] = [];
  private historyIndex = -1;
  private hiddenForPopup = false;
  private unregisterBlur: (() => void) | null = null;
  /// Last bounds sent, as "x,y,w,h", so the position check below only
  /// talks to the backend when something moved.
  private lastBounds = "";
  private positionTimer: ReturnType<typeof setInterval> | null = null;
  private readonly onWindowResize = (): void => this.scheduleFit();

  constructor(opts: EmbeddedBrowserPaneOptions) {
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
    this.urlInput.disabled = true;
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

    // Placeholder — the child webview is positioned over this area
    this.placeholder = document.createElement("div");
    this.placeholder.className = "embedded-browser-pane__placeholder";
    this.placeholder.style.flex = "1 1 auto";
    this.placeholder.style.minHeight = "0";
    this.placeholder.style.minWidth = "0";
    this.placeholder.style.background = "#1a2230";
    this.placeholder.style.position = "relative";
    // Let pointer events pass through to the embedded browser webview below.
    // Without this the main WebView2 HTML intercepts all clicks in this area
    // and the embedded child webview never receives them.
    this.placeholder.style.pointerEvents = "none";
    this.element.appendChild(this.placeholder);

    this.resizeObserver = new ResizeObserver(() => this.scheduleFit());
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
    this.url = initial;

    // Wait for the placeholder to have non-zero dimensions before spawning.
    if (this.placeholder.getBoundingClientRect().width <= 1) {
      await new Promise<void>((resolve) => {
        const ro = new ResizeObserver(() => {
          if (this.placeholder.getBoundingClientRect().width > 1) {
            ro.disconnect();
            resolve();
          }
        });
        ro.observe(this.placeholder);
        setTimeout(() => { ro.disconnect(); resolve(); }, 2000);
      });
    }

    const bounds = this.getPhysicalBounds();
    console.log("[EmbeddedBrowserPane] spawn", { url: initial, ...bounds, dpr: window.devicePixelRatio });

    this.spawned = true;
    this.urlInput.value = initial;
    this.urlInput.disabled = false;
    this.pushHistory(initial);

    // Fire and forget — the IPC reply may be delayed on Windows but the
    // webview is still created on the main thread.
    void api
      .createEmbeddedBrowser(this.id, initial, bounds.x, bounds.y, bounds.width, bounds.height)
      .catch((e) => console.error("[EmbeddedBrowserPane] create failed", e));

    this.unregisterBlur = registerBlurListener((blurred) => this.onPopupBlur(blurred));
    if (isPopupOpen()) {
      this.onPopupBlur(true);
    }

    // The webview is created asynchronously on the main thread, so bounds
    // sent before it exists are dropped, and the layout may have shifted in
    // the meantime (fonts, a tab strip, the side panel). Re-send a few times
    // once it is likely up.
    for (const ms of [50, 250, 1000]) {
      setTimeout(() => this.syncBounds(true), ms);
    }
    // ResizeObserver only reports size changes; a pane that moves without
    // resizing (a row above it grows, a strip appears) would leave the
    // webview behind, over this pane's address bar. A cheap position check
    // catches that.
    this.positionTimer = setInterval(() => this.syncBounds(false), 400);
    window.addEventListener("resize", this.onWindowResize);
  }

  /// Send the placeholder's current bounds to the webview: always when
  /// `force`, otherwise only if they changed since the last send.
  private syncBounds(force: boolean): void {
    if (!this.spawned || this.hiddenForPopup) return;
    const b = this.getPhysicalBounds();
    const key = `${b.x},${b.y},${b.width},${b.height}`;
    if (!force && key === this.lastBounds) return;
    this.lastBounds = key;
    void api.setEmbeddedBrowserBounds(this.id, b.x, b.y, b.width, b.height).catch(() => {});
  }

  private onPopupBlur(blurred: boolean): void {
    if (!this.spawned) return;
    this.hiddenForPopup = blurred;
    if (blurred) {
      void api.setEmbeddedBrowserVisible(this.id, false).catch(() => {});
    } else {
      // Restore the real bounds — Rust's set_embedded_browser_visible(true) is
      // a no-op; the frontend must re-emit them.
      void api.setEmbeddedBrowserVisible(this.id, true).catch(() => {});
      this.syncBounds(true);
    }
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
    this.opts.onFocus?.();
  }

  scheduleFit(): void {
    if (this.fitRaf !== null) return;
    this.fitRaf = requestAnimationFrame(() => {
      this.fitRaf = null;
      this.syncBounds(true);
    });
  }

  setTitle(title: string | null): void {
    const el = this.element.querySelector(".pane-title");
    if (el) el.textContent = title || t("browser.defaultTitle");
  }

  dispose(): void {
    this.cleanupLang();
    this.resizeObserver.disconnect();
    if (this.positionTimer !== null) {
      clearInterval(this.positionTimer);
      this.positionTimer = null;
    }
    window.removeEventListener("resize", this.onWindowResize);
    if (this.unregisterBlur) {
      this.unregisterBlur();
      this.unregisterBlur = null;
    }
    if (this.fitRaf !== null) {
      cancelAnimationFrame(this.fitRaf);
      this.fitRaf = null;
    }
    if (this.spawned) {
      this.spawned = false;
      void api.destroyEmbeddedBrowser(this.id).catch(() => {});
    }
    this.element.remove();
  }

  // ── Navigation ────────────────────────────────────────────────────────

  private navigate(raw: string): void {
    const url = normalizeUrl(raw);
    if (!url) return;
    this.url = url;
    this.urlInput.value = url;
    this.pushHistory(url);
    if (this.spawned) {
      void api.navigateEmbeddedBrowser(this.id, url).catch((e) =>
        console.error("[EmbeddedBrowserPane] navigate failed", e),
      );
    }
    this.opts.onUrlChange?.(url);
  }

  private goBack(): void {
    if (this.historyIndex <= 0) return;
    this.historyIndex -= 1;
    const url = this.history[this.historyIndex];
    this.url = url;
    this.urlInput.value = url;
    if (this.spawned) void api.navigateEmbeddedBrowser(this.id, url).catch(() => {});
    this.opts.onUrlChange?.(url);
  }

  private goForward(): void {
    if (this.historyIndex >= this.history.length - 1) return;
    this.historyIndex += 1;
    const url = this.history[this.historyIndex];
    this.url = url;
    this.urlInput.value = url;
    if (this.spawned) void api.navigateEmbeddedBrowser(this.id, url).catch(() => {});
    this.opts.onUrlChange?.(url);
  }

  private doReload(): void {
    if (this.spawned && this.url) {
      void api.navigateEmbeddedBrowser(this.id, this.url).catch(() => {});
    }
  }

  private pushHistory(url: string): void {
    this.history = this.history.slice(0, this.historyIndex + 1);
    this.history.push(url);
    this.historyIndex = this.history.length - 1;
  }

  // ── Bounds ────────────────────────────────────────────────────────────

  /// Return the placeholder rect in physical pixels relative to the main
  /// window's content area. Child webviews use window-relative coordinates,
  /// so no win.innerPosition() offset is needed.
  private getPhysicalBounds(): { x: number; y: number; width: number; height: number } {
    const scale = window.devicePixelRatio || 1;
    const r = this.placeholder.getBoundingClientRect();
    return {
      x: Math.round(r.left * scale),
      y: Math.round(r.top * scale),
      width: Math.max(1, Math.round(r.width * scale)),
      height: Math.max(1, Math.round(r.height * scale)),
    };
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
