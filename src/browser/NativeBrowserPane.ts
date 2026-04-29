// Native browser pane: uses a Tauri child Webview EMBEDDED inside the main
// window (not a separate OS window). Bypasses X-Frame-Options / CSP that
// limit the iframe-based BrowserPane.
//
// Layout: a URL bar (back / forward / reload / input) sits in the DOM, and
// the native webview is positioned to overlay the placeholder below it.
// Because the webview lives in the same window, it moves with it naturally.

import type { PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";
import { api } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";

export interface NativeBrowserPaneOptions {
  spec: PaneSpec;
  onFocus?: () => void;
  onUrlChange?: (url: string) => void;
}

const DEFAULT_URL = "about:blank";

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
  private opts: NativeBrowserPaneOptions;
  private cleanupLang: () => void;
  private history: string[] = [];
  private historyIndex = -1;

  constructor(opts: NativeBrowserPaneOptions) {
    this.id = opts.spec.id;
    this.opts = opts;
    this.url = opts.spec.url?.trim() || DEFAULT_URL;

    this.element = document.createElement("div");
    this.element.className = "pane browser-pane";
    this.element.tabIndex = 0;
    this.element.dataset.paneId = this.id;

    const titleEl = document.createElement("div");
    titleEl.className = "pane-title";
    titleEl.textContent = opts.spec.title || t("browser.defaultTitle");
    this.element.appendChild(titleEl);

    const nav = document.createElement("div");
    nav.className = "browser-pane__nav";

    this.backBtn = iconBtn("←", t("browser.back"), () => this.goBack());
    this.fwdBtn = iconBtn("→", t("browser.forward"), () => this.goForward());
    this.reloadBtn = iconBtn("⟳", t("browser.reload"), () => this.doReload());

    this.urlInput = document.createElement("input");
    this.urlInput.type = "text";
    this.urlInput.className = "browser-pane__url";
    this.urlInput.placeholder = "https://…";
    this.urlInput.value = this.url === DEFAULT_URL ? "" : this.url;
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

    this.placeholder = document.createElement("div");
    this.placeholder.className = "native-browser-pane__placeholder";
    this.placeholder.style.flex = "1 1 auto";
    this.placeholder.style.minHeight = "0";
    this.placeholder.style.minWidth = "0";
    this.placeholder.style.position = "relative";
    this.element.appendChild(this.placeholder);

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
    const initial = this.url === DEFAULT_URL ? DEFAULT_URL : normalizeUrl(this.url) ?? DEFAULT_URL;
    const rect = this.getRect();
    await api.createWebview(this.id, initial, rect.x, rect.y, rect.width, rect.height);
    this.spawned = true;
    if (initial !== DEFAULT_URL) this.pushHistory(initial);
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
    if (this.repositionRaf !== null) cancelAnimationFrame(this.repositionRaf);
    if (this.spawned) {
      void api.destroyWebview(this.id).catch(() => {});
      this.spawned = false;
    }
    this.element.remove();
  }

  private navigate(raw: string): void {
    const url = normalizeUrl(raw);
    if (!url) return;
    this.url = url;
    this.urlInput.value = url;
    this.pushHistory(url);
    if (this.spawned) void api.navigateWebview(this.id, url).catch(() => {});
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

  private scheduleReposition(): void {
    if (this.repositionRaf !== null) return;
    this.repositionRaf = requestAnimationFrame(() => {
      this.repositionRaf = null;
      void this.reposition().catch(() => {});
    });
  }

  private async reposition(): Promise<void> {
    if (!this.spawned) return;
    const rect = this.getRect();
    await api.resizeWebview(this.id, rect.x, rect.y, rect.width, rect.height);
  }

  private getRect(): { x: number; y: number; width: number; height: number } {
    const r = this.placeholder.getBoundingClientRect();
    return {
      x: Math.round(r.left),
      y: Math.round(r.top),
      width: Math.max(1, Math.round(r.width)),
      height: Math.max(1, Math.round(r.height)),
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
