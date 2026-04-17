// HotKey bar shown above every terminal pane. Renders one button per
// `HotKeyDef`, plus a `⚙` management button that opens the `HotKeyManager`
// modal. A click injects the command's bytes into the PTY via the existing
// `writePane` IPC — no new backend command required.

import type { HotKeyDef, Uuid } from "../types";
import { api } from "../ipc/bridge";
import { openHotKeyManager } from "../hotkey/HotKeyManager";

const ENCODER = new TextEncoder();

export interface HotKeyBarOptions {
  paneId: Uuid;
  initial: HotKeyDef[];
  /// Called whenever the hotkey list is mutated (add / edit / delete / reorder)
  /// so the WorkspaceManager can persist the change.
  onChange: (next: HotKeyDef[]) => void;
}

export class HotKeyBar {
  readonly element: HTMLElement;
  private hotkeys: HotKeyDef[];
  private opts: HotKeyBarOptions;

  constructor(opts: HotKeyBarOptions) {
    this.opts = opts;
    this.hotkeys = [...opts.initial];
    this.element = document.createElement("div");
    this.element.className = "hotkey-bar";
    this.render();
  }

  setHotKeys(next: HotKeyDef[]): void {
    this.hotkeys = [...next];
    this.render();
  }

  private render(): void {
    while (this.element.firstChild) this.element.removeChild(this.element.firstChild);

    for (const def of this.hotkeys) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "hotkey-bar__btn";
      btn.textContent = def.label || def.command.slice(0, 16);
      btn.title = def.command;
      btn.addEventListener("click", (ev) => {
        ev.preventDefault();
        void this.execute(def);
      });
      this.element.appendChild(btn);
    }

    const manage = document.createElement("button");
    manage.type = "button";
    manage.className = "hotkey-bar__btn hotkey-bar__btn--manage";
    manage.textContent = "⚙";
    manage.title = "Manage HotKeys";
    manage.addEventListener("click", (ev) => {
      ev.preventDefault();
      openHotKeyManager(this.hotkeys, (next) => {
        this.hotkeys = [...next];
        this.opts.onChange(this.hotkeys);
        this.render();
      });
    });
    this.element.appendChild(manage);
  }

  private async execute(def: HotKeyDef): Promise<void> {
    if (def.batch) {
      const lines = def.command.split(/\r?\n/).map((l) => l.trimEnd());
      for (const line of lines) {
        if (!line) continue;
        await api.writePane(this.opts.paneId, ENCODER.encode(`${line}\r`));
      }
    } else {
      // Non-batch: send the entire command (newlines included as `\r`) in one
      // shot, followed by a final `\r` so multi-line blocks are committed.
      const normalized = def.command.replace(/\r?\n/g, "\r");
      const terminated = normalized.endsWith("\r") ? normalized : `${normalized}\r`;
      await api.writePane(this.opts.paneId, ENCODER.encode(terminated));
    }
  }
}
