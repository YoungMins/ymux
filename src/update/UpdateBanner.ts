import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { api } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";

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
  host.querySelectorAll(".update-banner").forEach((n) => n.remove());

  const banner = document.createElement("div");
  banner.className = "update-banner";
  banner.setAttribute("role", "status");

  const label = document.createElement("span");
  label.textContent = `v${info.version} ${t("update.available")}`;
  banner.appendChild(label);

  const link = document.createElement("button");
  link.type = "button";
  link.className = "update-banner__link";
  link.textContent = t("update.releaseNotes");
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
  close.title = t("update.dismiss");
  close.addEventListener("click", () => {
    try {
      localStorage.setItem(DISMISS_KEY, info.version);
    } catch {
      // ignore
    }
    banner.remove();
  });
  banner.appendChild(close);

  const cleanup = onLangChange(() => {
    label.textContent = `v${info.version} ${t("update.available")}`;
    link.textContent = t("update.releaseNotes");
    close.title = t("update.dismiss");
  });

  const origRemove = banner.remove.bind(banner);
  banner.remove = () => { cleanup(); origRemove(); };

  host.appendChild(banner);
}
