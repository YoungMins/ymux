// Bottom-right banner that appears when the Rust updater emits
// `app:update-available`. Purely informational — we never auto-install.
// Dismiss is sticky per-version via localStorage so we don't nag users who
// already saw a specific release notice.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api } from "../ipc/bridge";

const UPDATE_EVENT = "app:update-available";
const DISMISS_KEY = "ymux:update-dismissed-version";

interface UpdateInfo {
  version: string;
  url: string;
  notes: string;
}

export async function mountUpdateBanner(host: HTMLElement): Promise<UnlistenFn> {
  return listen<UpdateInfo>(UPDATE_EVENT, (event) => {
    const info = event.payload;
    try {
      const dismissed = localStorage.getItem(DISMISS_KEY);
      if (dismissed === info.version) return;
    } catch {
      // localStorage disabled — show anyway.
    }
    renderBanner(host, info);
  });
}

function renderBanner(host: HTMLElement, info: UpdateInfo): void {
  // Remove any previous banner first: if the user shipped two releases before
  // dismissing, we want only the newest visible.
  host.querySelectorAll(".update-banner").forEach((n) => n.remove());

  const banner = document.createElement("div");
  banner.className = "update-banner";
  banner.setAttribute("role", "status");

  const label = document.createElement("span");
  label.textContent = `v${info.version} available`;
  banner.appendChild(label);

  const link = document.createElement("button");
  link.type = "button";
  link.className = "update-banner__link";
  link.textContent = "Release notes";
  link.addEventListener("click", () => {
    void api.openUrl(info.url).catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  banner.appendChild(link);

  const close = document.createElement("button");
  close.type = "button";
  close.className = "update-banner__close";
  close.textContent = "×";
  close.title = "Dismiss";
  close.addEventListener("click", () => {
    try {
      localStorage.setItem(DISMISS_KEY, info.version);
    } catch {
      // ignore
    }
    banner.remove();
  });
  banner.appendChild(close);

  host.appendChild(banner);
}
