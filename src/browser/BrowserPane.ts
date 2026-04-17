// Browser pane: embeds a URL in an iframe inside the layout tree. Designed to
// mirror `TerminalPane`'s public shape so `SplitContainer` and
// `WorkspaceManager` can treat every leaf uniformly through the `Pane`
// interface.
//
// Intentionally kept to a plain iframe for the MVP: many sites reject
// embedding via `X-Frame-Options` or CSP `frame-ancestors`, which we surface
// as an inline message after a load-timeout. A future upgrade could swap in
// Tauri 2's child `Webview` API (no CSP restrictions) at the cost of having
// to manage its geometry outside the normal DOM flow.

import type { PaneSpec, Uuid } from "../types";
import type { Pane } from "../layout/Pane";

export interface BrowserPaneOptions {
  spec: PaneSpec;
  onFocus?: () => void;
  /// Called whenever the user navigates to a new URL so the manager can
  /// persist it into the `PaneSpec`.
  onUrlChange?: (url: string) => void;
}

const DEFAULT_URL = "about:blank";

export class BrowserPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  private iframe: HTMLIFrameElement;
  private urlInput: HTMLInputElement;
  private history: string[] = [];
  private historyIndex = -1;
  private opts: BrowserPaneOptions;

  constructor(opts: BrowserPaneOptions) {
    this.id = opts.spec.id;
    this.opts = opts;

    this.element = document.createElement("div");
    this.element.className = "pane browser-pane";
    this.element.tabIndex = 0;
    this.element.dataset.paneId = this.id;

    const nav = document.createElement("div");
    nav.className = "browser-pane__nav";

    const backBtn = makeIconBtn("←", "Back", () => this.goBack());
    const fwdBtn = makeIconBtn("→", "Forward", () => this.goForward());
    const reloadBtn = makeIconBtn("⟳", "Reload", () => this.reload());

    this.urlInput = document.createElement("input");
    this.urlInput.type = "text";
    this.urlInput.className = "browser-pane__url";
    this.urlInput.placeholder = "https://…";
    this.urlInput.value = opts.spec.url ?? "";
    this.urlInput.spellcheck = false;
    this.urlInput.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        this.navigate(this.urlInput.value.trim());
      }
    });

    nav.appendChild(backBtn);
    nav.appendChild(fwdBtn);
    nav.appendChild(reloadBtn);
    nav.appendChild(this.urlInput);

    this.iframe = document.createElement("iframe");
    this.iframe.className = "browser-pane__iframe";
    // `allow-same-origin` is required so sites can load their own resources;
    // `allow-scripts` lets JS run; `allow-forms` and `allow-popups` cover
    // common dashboard use cases. No `allow-top-navigation` — the iframe must
    // never be able to replace the ymux window itself.
    this.iframe.setAttribute(
      "sandbox",
      "allow-scripts allow-same-origin allow-forms allow-popups",
    );
    this.iframe.referrerPolicy = "no-referrer";

    this.element.appendChild(nav);
    this.element.appendChild(this.iframe);

    this.element.addEventListener("focusin", () => this.opts.onFocus?.());
    this.element.addEventListener("pointerdown", () => this.focus());
  }

  async spawn(): Promise<void> {
    const initial = this.opts.spec.url?.trim() || DEFAULT_URL;
    this.navigate(initial, /* recordHistory */ true);
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
    this.opts.onFocus?.();
  }

  scheduleFit(): void {
    // iframe uses CSS sizing; nothing to do.
  }

  dispose(): void {
    this.iframe.src = "about:blank";
    this.element.remove();
  }

  private navigate(raw: string, recordHistory = true): void {
    if (!raw) return;
    const url = normalize(raw);
    if (!url) return;
    this.iframe.src = url;
    this.urlInput.value = url;
    if (recordHistory) {
      // Trim forward history when the user navigates from a middle state.
      this.history = this.history.slice(0, this.historyIndex + 1);
      this.history.push(url);
      this.historyIndex = this.history.length - 1;
    }
    this.opts.onUrlChange?.(url);
  }

  private goBack(): void {
    if (this.historyIndex <= 0) return;
    this.historyIndex -= 1;
    const url = this.history[this.historyIndex];
    this.iframe.src = url;
    this.urlInput.value = url;
    this.opts.onUrlChange?.(url);
  }

  private goForward(): void {
    if (this.historyIndex >= this.history.length - 1) return;
    this.historyIndex += 1;
    const url = this.history[this.historyIndex];
    this.iframe.src = url;
    this.urlInput.value = url;
    this.opts.onUrlChange?.(url);
  }

  private reload(): void {
    // Reassigning `src` to the same value is the reliable reload mechanism for
    // sandboxed iframes — `contentWindow.location.reload()` throws on
    // cross-origin documents.
    const current = this.iframe.src;
    this.iframe.src = "about:blank";
    requestAnimationFrame(() => {
      this.iframe.src = current;
    });
  }
}

function makeIconBtn(
  icon: string,
  title: string,
  onClick: () => void,
): HTMLButtonElement {
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

/// Normalize a user-typed URL: prepend https:// if there's no scheme, and
/// reject anything that parses to a non-http(s) protocol (iframes with
/// `javascript:` or `file:` would be a security issue).
function normalize(input: string): string | null {
  let candidate = input.trim();
  if (!candidate) return null;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(candidate)) {
    candidate = `https://${candidate}`;
  }
  try {
    const url = new URL(candidate);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}
