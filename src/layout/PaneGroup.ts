// The chrome of a tabbed pane. One `PaneGroup` per `LayoutNode::Tabs`, cached
// by the group's id (see WorkspaceManager) because `SplitContainer.render()`
// rebuilds the DOM on every layout mutation and this object owns a HotKeyBar
// with a language subscription.
//
// Layout, top to bottom (spec §4):
//
//   .pane-group
//   ├── .pane-title        ← the active tab's label
//   ├── .hotkey-bar        ← ONE bar, re-bound to the active tab on each switch
//   ├── .pane-tabs         ← the strip; hidden while the group has one tab
//   └── .pane-group__body  ← every child's `.pane` element, non-active ones
//                            carrying `.pane--tab-hidden` (display: none)
//
// Children are `TerminalPane`s built with `ownChrome: false`, so nothing here
// fights a pane over who draws the title. Re-parenting a child element is
// safe — that is how `SplitContainer` has always preserved xterm instances —
// but it resets `.xterm-viewport`'s scrollTop, so a tab being *shown* is
// re-fitted on the next animation frame (`scheduleFit` also re-syncs the
// scrollbar; see terminal/viewportSync.ts).

import type { HotKeyDef, Uuid } from "../types";
import type { Pane } from "./Pane";
import type { PaneNode, TabsNode } from "./tabs";
import { tabIds } from "./tabs";
import { HotKeyBar } from "../terminal/HotKeyBar";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { t, onLangChange } from "../i18n/i18n";

export interface PaneGroupCallbacks {
  /// Tab label: `tabLabel()` applied to the pane's spec, the process-scan
  /// label and the localised fallback. The manager owns those inputs.
  labelOf: (paneId: Uuid) => string;
  onSelectTab: (paneId: Uuid) => void;
  onNewTab: () => void;
  onCloseTab: (paneId: Uuid) => void;
  onCloseOthers: (paneId: Uuid) => void;
  onRenameTab: (paneId: Uuid) => void;
  onHotKeysChange: (paneId: Uuid, hotkeys: HotKeyDef[]) => void;
  onBgColorChange: (paneId: Uuid, color: string | null) => void;
  /// A hotkey wrote straight to the PTY, bypassing xterm's `onData`.
  onHotKeySubmit: (paneId: Uuid) => void;
}

export class PaneGroup {
  readonly element: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly strip: HTMLElement;
  private readonly body: HTMLElement;
  private readonly hotkeyBar: HotKeyBar;
  private readonly cleanupLang: () => void;
  /// The tab the chrome is currently bound to, so a re-render that did not
  /// change the active tab costs no HotKeyBar rebuild.
  private boundId: Uuid | null = null;
  private node: TabsNode | null = null;

  constructor(
    readonly id: Uuid,
    private readonly cb: PaneGroupCallbacks,
  ) {
    this.element = document.createElement("div");
    this.element.className = "pane-group";
    this.element.dataset.groupId = id;

    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.element.appendChild(this.titleEl);

    // One bar for the whole group. `paneId` is a placeholder until the first
    // `update()` binds it — nothing can click it before then, because the
    // element is not in the document yet.
    this.hotkeyBar = new HotKeyBar({
      paneId: id,
      initial: [],
      initialBgColor: null,
      onSubmit: () => {
        if (this.boundId) this.cb.onHotKeySubmit(this.boundId);
      },
      onChange: (next) => {
        if (this.boundId) this.cb.onHotKeysChange(this.boundId, next);
      },
      onBgColorChange: (color) => {
        if (this.boundId) this.cb.onBgColorChange(this.boundId, color);
      },
    });
    this.element.appendChild(this.hotkeyBar.element);

    this.strip = document.createElement("div");
    this.strip.className = "pane-tabs";
    this.element.appendChild(this.strip);

    this.body = document.createElement("div");
    this.body.className = "pane-group__body";
    this.element.appendChild(this.body);

    // Labels are program names, but the `+` button's tooltip and the context
    // menu are translated, so a language switch has to repaint the strip.
    this.cleanupLang = onLangChange(() => {
      if (this.node) this.renderStrip(this.node);
    });
  }

  /// Re-render for `node`. Called on every layout render; cheap when nothing
  /// moved, because the strip is a handful of buttons and the child elements
  /// are re-appended, not rebuilt.
  update(node: TabsNode, paneCache: Map<Uuid, Pane>): void {
    this.node = node;
    const ids = tabIds(node);
    const activeId = ids[node.active] ?? ids[0] ?? null;

    this.titleEl.textContent = activeId ? this.cb.labelOf(activeId) : "";
    this.renderStrip(node);

    if (activeId && activeId !== this.boundId) {
      // The child nodes *are* pane specs, so the bar's list and colour come
      // straight off the tree — no lookup through the manager needed.
      const spec = node.children.find(
        (c): c is PaneNode => c.kind === "pane" && c.id === activeId,
      );
      this.boundId = activeId;
      this.hotkeyBar.bind(activeId, spec?.hotkeys ?? [], spec?.bg_color || null);
    }

    this.body.replaceChildren();
    for (const paneId of ids) {
      const pane = paneCache.get(paneId);
      if (!pane) continue;
      const hidden = paneId !== activeId;
      pane.element.classList.toggle("pane--tab-hidden", hidden);
      this.body.appendChild(pane.element);
      if (!hidden) {
        // The element was just (re-)attached and un-hidden. Its layout box is
        // new, so `.xterm-viewport`'s scrollTop is 0 behind xterm's back and
        // its size may have changed while it was hidden. One frame later the
        // box is measurable; `scheduleFit` then fits AND re-syncs the
        // scrollbar (viewportSync.ts), which is what stops the next wheel
        // notch jumping to the top of the scrollback.
        requestAnimationFrame(() => pane.scheduleFit());
      }
    }
  }

  /// Re-draw the title and the strip from the callbacks, leaving the body
  /// alone. The 2 s process scan changes labels often — every command that
  /// starts or finishes — and a full `SplitContainer.render()` would detach
  /// and re-attach every terminal that often, resetting scrollbars and
  /// forcing refits on panes that never moved.
  refreshLabels(): void {
    if (!this.node) return;
    const ids = tabIds(this.node);
    const activeId = ids[this.node.active] ?? ids[0] ?? null;
    this.titleEl.textContent = activeId ? this.cb.labelOf(activeId) : "";
    this.renderStrip(this.node);
  }

  dispose(): void {
    this.cleanupLang();
    this.hotkeyBar.dispose();
    this.element.remove();
  }

  private renderStrip(node: TabsNode): void {
    const ids = tabIds(node);
    // Spec §4: "tab strip (hidden when only one tab)" — a pane with one tab
    // must look exactly like a pane does today.
    this.strip.style.display = ids.length > 1 ? "" : "none";
    this.strip.replaceChildren();

    ids.forEach((paneId, idx) => {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = "pane-tabs__tab";
      tab.dataset.paneId = paneId;
      if (idx === node.active) tab.classList.add("pane-tabs__tab--active");

      const text = this.cb.labelOf(paneId);
      const label = document.createElement("span");
      label.className = "pane-tabs__label";
      label.textContent = text;
      tab.appendChild(label);
      tab.title = text;

      const close = document.createElement("span");
      close.className = "pane-tabs__close";
      close.textContent = "×";
      close.title = t("tab.close");
      close.addEventListener("click", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        this.cb.onCloseTab(paneId);
      });
      tab.appendChild(close);

      tab.addEventListener("click", () => this.cb.onSelectTab(paneId));
      tab.addEventListener("dblclick", (ev) => {
        ev.preventDefault();
        this.cb.onRenameTab(paneId);
      });
      tab.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        const entries: ContextMenuEntry[] = [
          { label: t("tab.rename"), onSelect: () => this.cb.onRenameTab(paneId) },
          "separator",
          { label: t("tab.close"), onSelect: () => this.cb.onCloseTab(paneId) },
          {
            label: t("tab.closeOthers"),
            disabled: ids.length < 2,
            onSelect: () => this.cb.onCloseOthers(paneId),
          },
        ];
        showContextMenu(ev.clientX, ev.clientY, entries);
      });
      this.strip.appendChild(tab);
    });

    const add = document.createElement("button");
    add.type = "button";
    add.className = "pane-tabs__add";
    add.textContent = "+";
    add.title = t("tab.new");
    add.setAttribute("aria-label", t("tab.new"));
    add.addEventListener("click", () => this.cb.onNewTab());
    this.strip.appendChild(add);
  }
}
