# Workspace Left Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the workspace list from the top bar into a full-height, scrollable, collapsible left panel, keeping the other top-bar controls in place and terminals resizing cleanly on collapse.

**Architecture:** `#app` becomes a horizontal row of `.workspace-panel` (new, full height) and `.app-main` (the existing vertical stack: top bar, panes, status bar). A new `WorkspacePanel.ts` owns the vertical list; `WorkspaceBar.ts` is slimmed to the right-side controls plus a panel-toggle button. Collapse persists in `localStorage`; toggling explicitly refits terminals so a width change behaves like a window resize.

**Tech Stack:** TypeScript, xterm.js (FitAddon), vanilla DOM, Vite, Vitest.

## Global Constraints

- No new dependencies.
- All user-visible strings go through `src/i18n/i18n.ts`, 13 languages: en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi (CLAUDE.md rule #7).
- Every task ends `npx tsc --noEmit` clean and (where tests exist) `pnpm exec vitest run` green.
- Preserve all existing behavior: switch (click), rename (dblclick), notes toggle, delete (hidden when one workspace), status dots.
- Terminals refit ONLY via `manager.refitActive()` (no per-pane ResizeObserver); the panel toggle MUST call it inside `requestAnimationFrame`, and the panel width change MUST be instant (no CSS width transition).
- DRY, YAGNI, TDD, commit after every task.

Key existing anchors (verified against the tree at branch `claude/workspace-left-panel`):
- `src/workspace/WorkspaceBar.ts` — current `mountWorkspaceBar(host, manager, shells)`, `makePair`, `rebuild`, `highlight`, `refreshWorkspaceBar(host)` with the `__ymuxHighlight` stash (lines 244/254-260).
- `src/main.ts:33-41` — builds `.workspace-host`, `new WorkspaceManager(host, …)`, `mountWorkspaceBar(app, …)`, then moves the bar above the host; `main.ts:52` `mountStatusBar(app)`; `main.ts:101,112` `refreshWorkspaceBar(app)`.
- `src/workspace/WorkspaceManager.ts` — `refitActive()` (line ~938-943), `activate(id)`, `addWorkspace()`, `deleteWorkspace(id)`, `renameWorkspace(id,name)`, `getWorkspaceName(id)`, `activeIdValue`, `workspaces`, `workspaceStatus(id)`, `onWorkspacesChange(cb)`, `onPaneStatusChange` (assignable), `setDefaultShell`, `splitFocusedBrowser`.
- `src/notes/NotesOverlay.ts` — `toggle`, `hasNotes`, `onNotesChange`; localStorage try/catch pattern.
- `src/i18n/i18n.ts:350` — `workspace.addWorkspace` entry shows the 13-key object shape.
- `src/style.css:27-48` — `#app { display:flex; flex-direction:column }` and `.workspace-bar`.

---

## Task 1: Pure label + sort helpers (TDD)

**Files:**
- Create: `src/workspace/workspaceLabel.ts`
- Test: `src/workspace/workspaceLabel.test.ts`

**Interfaces:**
- Produces:
  - `formatWorkspaceLabel(id: number, name: string | null | undefined): string`
  - `sortWorkspacesById<T extends { id: number }>(list: readonly T[]): T[]`

- [ ] **Step 1: Write the failing test**

Create `src/workspace/workspaceLabel.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { formatWorkspaceLabel, sortWorkspacesById } from "./workspaceLabel";

describe("formatWorkspaceLabel", () => {
  it("shows just the id when there is no custom name", () => {
    expect(formatWorkspaceLabel(3, null)).toBe("3");
    expect(formatWorkspaceLabel(3, undefined)).toBe("3");
    expect(formatWorkspaceLabel(3, "")).toBe("3");
  });

  it("treats the default names as no custom name", () => {
    expect(formatWorkspaceLabel(2, "workspace-2")).toBe("2");
    expect(formatWorkspaceLabel(1, "main")).toBe("1");
  });

  it("shows 'id: name' for a custom name", () => {
    expect(formatWorkspaceLabel(1, "build")).toBe("1: build");
  });
});

describe("sortWorkspacesById", () => {
  it("returns a new array sorted ascending by id, leaving the input untouched", () => {
    const input = [{ id: 3 }, { id: 1 }, { id: 2 }];
    const out = sortWorkspacesById(input);
    expect(out.map((w) => w.id)).toEqual([1, 2, 3]);
    expect(input.map((w) => w.id)).toEqual([3, 1, 2]);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm exec vitest run src/workspace/workspaceLabel.test.ts`
Expected: FAIL — cannot resolve `./workspaceLabel`.

- [ ] **Step 3: Write the implementation**

Create `src/workspace/workspaceLabel.ts`:

```ts
/// A workspace's `name` is "custom" only when the user actually renamed it —
/// the auto-assigned `workspace-<id>` and the legacy "main" default do not
/// count, so those render as just the number.
function isCustomName(id: number, name: string | null | undefined): name is string {
  return !!name && name !== `workspace-${id}` && name !== "main";
}

/// Label for a workspace tab/row: `"1: build"` when custom-named, else `"1"`.
export function formatWorkspaceLabel(
  id: number,
  name: string | null | undefined,
): string {
  return isCustomName(id, name) ? `${id}: ${name}` : String(id);
}

/// Ascending-by-id copy of a workspace list, so the panel order is stable and
/// independent of Map/insertion order. Does not mutate the input.
export function sortWorkspacesById<T extends { id: number }>(
  list: readonly T[],
): T[] {
  return [...list].sort((a, b) => a.id - b.id);
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm exec vitest run src/workspace/workspaceLabel.test.ts`
Expected: PASS (5 tests).
Then: `npx tsc --noEmit` → clean.

- [ ] **Step 5: Commit**

```bash
git add src/workspace/workspaceLabel.ts src/workspace/workspaceLabel.test.ts
git commit -m "feat(workspace): pure label + sort helpers for the panel"
```

---

## Task 2: i18n keys for the panel

**Files:**
- Modify: `src/i18n/i18n.ts` (add two entries next to `workspace.addWorkspace` at :350)

**Interfaces:**
- Produces: i18n keys `workspace.panelTitle`, `workspace.togglePanel` (read via `t(...)`).

- [ ] **Step 1: Add the two keys**

In `src/i18n/i18n.ts`, immediately before the `"workspace.addWorkspace": {` entry, insert:

```ts
  "workspace.panelTitle": {
    en: "Workspaces", ko: "작업공간", ja: "ワークスペース",
    zh: "工作区", hi: "कार्यक्षेत्र", es: "Espacios",
    fr: "Espaces", ar: "مساحات العمل", pt: "Espaços",
    ru: "Пространства", tr: "Çalışma alanları", de: "Arbeitsbereiche", vi: "Không gian",
  },
  "workspace.togglePanel": {
    en: "Toggle workspace panel", ko: "작업공간 패널 토글", ja: "ワークスペースパネルの切り替え",
    zh: "切换工作区面板", hi: "कार्यक्षेत्र पैनल टॉगल करें", es: "Alternar panel de espacios",
    fr: "Basculer le panneau des espaces", ar: "تبديل لوحة مساحات العمل", pt: "Alternar painel de espaços",
    ru: "Переключить панель пространств", tr: "Çalışma alanı panelini aç/kapat", de: "Arbeitsbereichsleiste umschalten", vi: "Bật/tắt bảng không gian",
  },
```

- [ ] **Step 2: Verify**

Run: `npx tsc --noEmit` → clean.

- [ ] **Step 3: Commit**

```bash
git add src/i18n/i18n.ts
git commit -m "feat(i18n): add workspace panel title + toggle strings (13 langs)"
```

---

## Task 3: `WorkspacePanel.ts` — the vertical list

**Files:**
- Create: `src/workspace/WorkspacePanel.ts`

**Interfaces:**
- Consumes: `formatWorkspaceLabel`, `sortWorkspacesById` (Task 1); i18n keys (Task 2); `WorkspaceManager` members listed in Global Constraints; `toggle`/`hasNotes`/`onNotesChange` from NotesOverlay; `promptWithBlur`/`confirmWithBlur` from popupBlur.
- Produces:
  - `mountWorkspacePanel(host: HTMLElement, manager: WorkspaceManager): () => void`
  - `refreshWorkspacePanel(host: HTMLElement): void`
  - `toggleWorkspacePanel(manager: WorkspaceManager): void`

- [ ] **Step 1: Create the file**

Create `src/workspace/WorkspacePanel.ts` with the full content below. It mirrors the current `WorkspaceBar` list logic (`makePair`→`makeRow`, `rebuild`, `highlight`) but lays rows out vertically, adds a header + sticky add button, and manages the collapsed state.

```ts
import type { WorkspaceManager } from "./WorkspaceManager";
import { formatWorkspaceLabel, sortWorkspacesById } from "./workspaceLabel";
import {
  toggle as toggleNotes,
  hasNotes,
  onNotesChange,
} from "../notes/NotesOverlay";
import { t, onLangChange } from "../i18n/i18n";
import { promptWithBlur, confirmWithBlur } from "../browser/popupBlur";

const COLLAPSE_KEY = "ymux:workspace-panel:collapsed";

const noteIconSvg = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>`;

function wsTooltip(id: number, manager: WorkspaceManager): string {
  const name = manager.getWorkspaceName(id);
  const base = name ? `${id}: ${name}` : `Workspace ${id}`;
  return `${base} (Ctrl+Alt+${id}) — ${t("workspace.dblclickRename")}`;
}

/// Read the persisted collapsed flag (default: expanded). localStorage may
/// throw in some webview contexts, so treat any failure as "expanded".
function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(COLLAPSE_KEY, collapsed ? "1" : "0");
  } catch {
    /* localStorage unavailable — collapse just won't persist */
  }
}

export function mountWorkspacePanel(
  host: HTMLElement,
  manager: WorkspaceManager,
): () => void {
  const panel = document.createElement("div");
  panel.className = "workspace-panel";
  if (readCollapsed()) panel.classList.add("workspace-panel--collapsed");

  const header = document.createElement("div");
  header.className = "workspace-panel__header";
  header.textContent = t("workspace.panelTitle");
  panel.appendChild(header);

  const list = document.createElement("div");
  list.className = "workspace-panel__list";
  panel.appendChild(list);

  const addBtn = document.createElement("button");
  addBtn.className = "workspace-panel__add";
  addBtn.type = "button";
  addBtn.textContent = "+";
  addBtn.title = t("workspace.addWorkspace");
  addBtn.setAttribute("aria-label", t("workspace.addWorkspace"));
  addBtn.addEventListener("click", () => {
    void manager.addWorkspace(); // fires onWorkspacesChange → rebuild()
  });
  panel.appendChild(addBtn);

  const buttons = new Map<number, HTMLButtonElement>();
  const noteButtons = new Map<number, HTMLButtonElement>();
  const statusDots = new Map<number, HTMLElement>();

  /// Build one vertical row: switch button (label + status dot) with a note
  /// button and a delete button trailing it.
  function makeRow(id: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "workspace-panel__row";

    const btn = document.createElement("button");
    btn.className = "workspace-panel__ws";
    btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
    btn.title = wsTooltip(id, manager);
    btn.addEventListener("click", () => {
      void manager.activate(id);
      highlight();
    });
    btn.addEventListener("dblclick", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const current = manager.getWorkspaceName(id) ?? "";
      const next = promptWithBlur(t("workspace.renamePrompt"), current);
      if (next !== null) {
        manager.renameWorkspace(id, next);
        highlight();
      }
    });

    const statusDot = document.createElement("span");
    statusDot.className = "ws-dot ws-dot--idle";
    btn.appendChild(statusDot);
    statusDots.set(id, statusDot);
    row.appendChild(btn);
    buttons.set(id, btn);

    const noteBtn = document.createElement("button");
    noteBtn.className = "workspace-panel__note-btn";
    noteBtn.type = "button";
    noteBtn.innerHTML = noteIconSvg;
    noteBtn.title = `${t("notes.title")} — ${id}`;
    noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${id}`);
    noteBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleNotes(id, manager.getWorkspaceName(id));
    });
    row.appendChild(noteBtn);
    noteButtons.set(id, noteBtn);

    const delBtn = document.createElement("button");
    delBtn.className = "workspace-panel__del";
    delBtn.type = "button";
    delBtn.textContent = "×";
    delBtn.title = t("workspace.deleteWorkspace");
    delBtn.setAttribute("aria-label", t("workspace.deleteWorkspace"));
    if (manager.workspaces.length <= 1) delBtn.style.display = "none";
    delBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (manager.workspaces.length <= 1) return;
      const label = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      const msg = t("workspace.deleteConfirm").replace("{name}", label);
      if (confirmWithBlur(msg)) {
        void manager.deleteWorkspace(id); // fires onWorkspacesChange → rebuild()
      }
    });
    row.appendChild(delBtn);

    return row;
  }

  function rebuild(): void {
    buttons.clear();
    noteButtons.clear();
    statusDots.clear();
    while (list.firstChild) list.removeChild(list.firstChild);
    for (const ws of sortWorkspacesById(manager.workspaces)) {
      list.appendChild(makeRow(ws.id));
    }
    highlight();
  }

  function highlight(): void {
    for (const [id, btn] of buttons) {
      btn.classList.toggle("workspace-panel__ws--active", id === manager.activeIdValue);
      btn.title = wsTooltip(id, manager);
      btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      const dot = statusDots.get(id);
      if (dot) btn.appendChild(dot); // textContent above wiped children
    }
    for (const [id, dot] of statusDots) {
      const status = manager.workspaceStatus(id);
      dot.className = `ws-dot ws-dot--${status}`;
      dot.title = status === "idle" ? "" : t(`status.${status}`);
    }
    for (const [id, noteBtn] of noteButtons) {
      const label = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      noteBtn.title = `${t("notes.title")} — ${label}`;
      noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${label}`);
      noteBtn.classList.toggle("workspace-panel__note-btn--has-notes", hasNotes(id));
    }
  }

  manager.onWorkspacesChange(rebuild);
  manager.onPaneStatusChange = () => highlight();
  const cleanupNotesSub = onNotesChange(() => highlight());
  const cleanupLang = onLangChange(() => {
    header.textContent = t("workspace.panelTitle");
    addBtn.title = t("workspace.addWorkspace");
    addBtn.setAttribute("aria-label", t("workspace.addWorkspace"));
    rebuild();
  });

  host.appendChild(panel);
  rebuild();

  (panel as unknown as { __ymuxHighlight: () => void }).__ymuxHighlight = highlight;

  return () => {
    cleanupLang();
    cleanupNotesSub();
    panel.remove();
  };
}

/// Re-run the panel's highlight pass (active state, labels, status dots,
/// has-notes) — mirrors the old refreshWorkspaceBar so main.ts's keyboard
/// paths can force an update.
export function refreshWorkspacePanel(host: HTMLElement): void {
  const panel = host.querySelector<HTMLElement>(".workspace-panel");
  if (!panel) return;
  (panel as unknown as { __ymuxHighlight?: () => void }).__ymuxHighlight?.();
}

/// Collapse/expand the panel and persist the choice. The width change is
/// instant (CSS toggles display), so on the next animation frame we refit the
/// active workspace's terminals — the same fit + ConPTY-resize path a window
/// resize uses — preventing mis-sized / garbled TUI redraws.
export function toggleWorkspacePanel(manager: WorkspaceManager): void {
  const panel = document.querySelector<HTMLElement>(".workspace-panel");
  if (!panel) return;
  const collapsed = panel.classList.toggle("workspace-panel--collapsed");
  writeCollapsed(collapsed);
  requestAnimationFrame(() => manager.refitActive());
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npx tsc --noEmit` → clean (the file is not yet imported anywhere; it must still type-check).

- [ ] **Step 3: Commit**

```bash
git add src/workspace/WorkspacePanel.ts
git commit -m "feat(workspace): WorkspacePanel — vertical scrollable list + collapse"
```

---

## Task 4: Restructure `main.ts` layout + wiring

Done BEFORE slimming the bar so every task leaves `tsc` green: here `main.ts`
stops importing `refreshWorkspaceBar` and mounts the panel, while the (still
fat) `WorkspaceBar` keeps exporting its old symbols. The app transiently shows
BOTH the old horizontal list and the new panel — removed in Task 5.

**Files:**
- Modify: `src/main.ts`

**Interfaces:**
- Consumes: `mountWorkspacePanel`, `refreshWorkspacePanel` (Task 3); `mountWorkspaceBar` (unchanged, Task 5 slims it).

- [ ] **Step 1: Swap the import**

In `src/main.ts`, change line 9 from:

```ts
import { mountWorkspaceBar, refreshWorkspaceBar } from "./workspace/WorkspaceBar";
```

to:

```ts
import { mountWorkspaceBar } from "./workspace/WorkspaceBar";
import { mountWorkspacePanel, refreshWorkspacePanel } from "./workspace/WorkspacePanel";
```

- [ ] **Step 2: Restructure the mount block**

Replace the current block (`main.ts:33-43`, from `const host = document.createElement("div");` through `await manager.start();`) with:

```ts
  // Left workspace panel (full height) + main column (top bar, panes, status).
  const panelEl = document.createElement("div");
  panelEl.className = "workspace-panel-host";
  const appMain = document.createElement("div");
  appMain.className = "app-main";
  app.appendChild(panelEl);
  app.appendChild(appMain);

  const host = document.createElement("div");
  host.className = "workspace-host";
  appMain.appendChild(host);

  const manager = new WorkspaceManager(host, bootstrap.config, bootstrap.shells);
  mountWorkspaceBar(appMain, manager, bootstrap.shells);
  // The bar was appended after the host; move it to the top of the column.
  const bar = appMain.querySelector(".workspace-bar");
  if (bar) appMain.insertBefore(bar, host);

  mountWorkspacePanel(panelEl, manager);

  await manager.start();
```

- [ ] **Step 3: Point the status bar at the main column**

In `src/main.ts`, change `mountStatusBar(app)` (line ~52) to `mountStatusBar(appMain)`:

```ts
  void mountStatusBar(appMain).catch((e) =>
    console.warn("mountStatusBar failed:", e),
  );
```

- [ ] **Step 4: Fix the two refresh call sites**

In `src/main.ts`, replace both `refreshWorkspaceBar(app)` calls (lines ~101 and ~112) with `refreshWorkspacePanel(app)`:

```ts
        void manager.activate(id).then(() => refreshWorkspacePanel(app));
```

and

```ts
      refreshWorkspacePanel(app);
```

- [ ] **Step 5: Verify it compiles**

Run: `npx tsc --noEmit` → clean (WorkspaceBar still exports its old symbols; nothing references the removed `refreshWorkspaceBar` import anymore).
Run: `pnpm exec vitest run` → green.

- [ ] **Step 6: Commit**

```bash
git add src/main.ts
git commit -m "feat(workspace): mount left panel + main column, wire refresh"
```

---

## Task 5: Slim `WorkspaceBar.ts` + add the panel toggle

**Files:**
- Modify: `src/workspace/WorkspaceBar.ts`

**Interfaces:**
- Consumes: `toggleWorkspacePanel` (Task 3), `t` (Task 2).
- Produces: `mountWorkspaceBar(host, manager, shells)` unchanged signature but the bar no longer renders the workspace list; a toggle button is prepended. `refreshWorkspaceBar`/`__ymuxHighlight` are removed (main.ts already switched to `refreshWorkspacePanel` in Task 4).

- [ ] **Step 1: Replace the file**

Overwrite `src/workspace/WorkspaceBar.ts` with:

```ts
import type { ShellProfile } from "../types";
import type { WorkspaceManager } from "./WorkspaceManager";
import { api } from "../ipc/bridge";
import { mountSettings } from "../settings/SettingsOverlay";
import { toggleWorkspacePanel } from "./WorkspacePanel";
import { t, onLangChange } from "../i18n/i18n";

const panelToggleSvg = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><line x1="9" y1="4" x2="9" y2="20"/></svg>`;

export function mountWorkspaceBar(
  host: HTMLElement,
  manager: WorkspaceManager,
  shells: ShellProfile[],
): () => void {
  const bar = document.createElement("div");
  bar.className = "workspace-bar";

  // Panel toggle — far left, single source of truth for show/hide.
  const toggleBtn = document.createElement("button");
  toggleBtn.className = "workspace-bar__icon-btn";
  toggleBtn.type = "button";
  toggleBtn.innerHTML = panelToggleSvg;
  toggleBtn.title = t("workspace.togglePanel");
  toggleBtn.setAttribute("aria-label", t("workspace.togglePanel"));
  toggleBtn.addEventListener("click", () => toggleWorkspacePanel(manager));
  bar.appendChild(toggleBtn);

  const spacer = document.createElement("div");
  spacer.className = "workspace-bar__spacer";
  bar.appendChild(spacer);

  const shellPicker = document.createElement("select");
  shellPicker.className = "workspace-bar__shell";
  shellPicker.title = t("workspace.shellTitle");
  for (const s of shells) {
    const opt = document.createElement("option");
    opt.value = s.name;
    opt.textContent = s.name;
    shellPicker.appendChild(opt);
  }
  shellPicker.addEventListener("change", () => {
    manager.setDefaultShell(shellPicker.value);
  });
  if (shells.length > 0) shellPicker.value = shells[0].name;
  bar.appendChild(shellPicker);

  const browserBtn = document.createElement("button");
  browserBtn.className = "workspace-bar__shell";
  browserBtn.type = "button";
  browserBtn.textContent = t("workspace.addBrowser");
  browserBtn.title = t("workspace.addBrowserTitle");
  browserBtn.style.cursor = "pointer";
  browserBtn.addEventListener("click", () => {
    void manager.splitFocusedBrowser("horizontal");
  });
  bar.appendChild(browserBtn);

  const kofiBtn = document.createElement("button");
  kofiBtn.className = "workspace-bar__icon-btn";
  kofiBtn.type = "button";
  kofiBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 8h2a2 2 0 0 1 0 4h-2"/><path d="M3 8h14v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V8z"/><line x1="6" y1="2" x2="6" y2="6"/><line x1="10" y1="2" x2="10" y2="6"/><line x1="14" y1="2" x2="14" y2="6"/></svg>`;
  kofiBtn.title = "Support on Ko-fi";
  kofiBtn.addEventListener("click", () => {
    void api.openUrl("https://ko-fi.com/youngminkim").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(kofiBtn);

  const ghBtn = document.createElement("button");
  ghBtn.className = "workspace-bar__icon-btn";
  ghBtn.type = "button";
  ghBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>`;
  ghBtn.title = "GitHub";
  ghBtn.addEventListener("click", () => {
    void api.openUrl("https://github.com/youngmins/ymux").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(ghBtn);

  const cleanupHelp = mountSettings(bar, manager);

  host.appendChild(bar);

  const cleanupLang = onLangChange(() => {
    toggleBtn.title = t("workspace.togglePanel");
    toggleBtn.setAttribute("aria-label", t("workspace.togglePanel"));
    shellPicker.title = t("workspace.shellTitle");
    browserBtn.textContent = t("workspace.addBrowser");
    browserBtn.title = t("workspace.addBrowserTitle");
    kofiBtn.title = t("workspace.supportTitle");
  });

  return () => {
    cleanupLang();
    cleanupHelp();
    bar.remove();
  };
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npx tsc --noEmit` → clean (main.ts already switched to `refreshWorkspacePanel` in Task 4, so nothing references the removed `refreshWorkspaceBar`).
Run: `pnpm exec vitest run` → green.

- [ ] **Step 3: Commit**

```bash
git add src/workspace/WorkspaceBar.ts
git commit -m "refactor(workspace): slim the top bar to controls + panel toggle"
```

---

## Task 6: CSS — panel layout, scroll, collapse

**Files:**
- Modify: `src/style.css`

**Interfaces:**
- Consumes: the class names emitted in Tasks 3-5 (`.app-main`, `.workspace-panel-host`, `.workspace-panel`, `.workspace-panel__header/__list/__add/__row/__ws/__note-btn/__del`, `.workspace-panel--collapsed`).

- [ ] **Step 1: Make `#app` a row and add the main column**

In `src/style.css`, replace the `#app` rule (lines 27-31):

```css
#app {
  display: flex;
  flex-direction: column;
  height: 100vh;
}
```

with:

```css
#app {
  display: flex;
  flex-direction: row;
  height: 100vh;
}

.app-main {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
  height: 100vh;
}
```

- [ ] **Step 2: Add the panel styles**

Append to `src/style.css`:

```css
/* Workspace left panel ------------------------------------------------ */
.workspace-panel-host {
  display: contents;
}

.workspace-panel {
  flex: 0 0 160px;
  width: 160px;
  height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-alt);
  border-right: 1px solid var(--border);
  user-select: none;
}

.workspace-panel--collapsed {
  display: none;
}

.workspace-panel__header {
  flex: 0 0 auto;
  height: 28px;
  display: flex;
  align-items: center;
  padding: 0 10px;
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--fg-muted);
  border-bottom: 1px solid var(--border);
}

.workspace-panel__list {
  flex: 1 1 auto;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.workspace-panel__row {
  display: flex;
  align-items: center;
  gap: 1px;
}

.workspace-panel__ws {
  position: relative;
  flex: 1 1 auto;
  min-width: 0;
  height: 24px;
  padding: 0 8px;
  text-align: left;
  background: transparent;
  color: var(--fg);
  border: 1px solid transparent;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.workspace-panel__ws:hover {
  background: var(--bg-hover);
}

.workspace-panel__ws--active {
  background: var(--bg-hover);
  color: var(--accent);
  border-color: var(--accent);
}

.workspace-panel__note-btn,
.workspace-panel__del {
  flex: 0 0 auto;
  background: transparent;
  color: var(--fg-muted);
  border: 1px solid transparent;
  border-radius: 4px;
  width: 20px;
  height: 22px;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  opacity: 0.45;
}

.workspace-panel__note-btn:hover,
.workspace-panel__del:hover {
  opacity: 1;
  border-color: var(--accent);
  color: var(--accent);
}

.workspace-panel__note-btn--has-notes {
  opacity: 1;
  color: var(--accent);
}

.workspace-panel__del {
  font-size: 15px;
  line-height: 1;
}

.workspace-panel__del:hover {
  border-color: var(--status-critical);
  color: var(--status-critical);
}

.workspace-panel__add {
  flex: 0 0 auto;
  height: 30px;
  background: transparent;
  color: var(--fg-muted);
  border: none;
  border-top: 1px solid var(--border);
  font-size: 16px;
  font-weight: 600;
  line-height: 1;
  cursor: pointer;
}

.workspace-panel__add:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
```

- [ ] **Step 3: Verify build + visual**

Run: `npx tsc --noEmit` → clean.
Run: `pnpm exec vitest run` → green.

- [ ] **Step 4: Manual GUI smoke (`pnpm tauri dev`, human — deferred if no Windows GUI)**

Confirm:
1. Workspaces render as a **vertical list** in the left panel; the top bar shows only the toggle + shell/browser/Ko-fi/GitHub/Settings.
2. Add ~15 workspaces → the list **scrolls**; header and `+` stay fixed.
3. Click switches; double-click renames; note button toggles notes; delete `×` works and is hidden with one workspace; status dots update.
4. The toggle button hides/shows the panel; state **persists across an app restart**.
5. **Toggle does not garble a TUI app:** open Claude Code (or another full-screen TUI) in a pane, collapse then expand the panel, confirm the app redraws cleanly at the new width (no overlapping text).

If `pnpm tauri dev` can't run here, report the smoke as deferred to a human; `tsc` + `vitest` are the CI-level guarantee.

- [ ] **Step 5: Commit**

```bash
git add src/style.css
git commit -m "feat(workspace): left panel styling, scroll, and collapse"
```

---

## Notes

- **Removed API:** `refreshWorkspaceBar` and the bar's `__ymuxHighlight` stash are gone; `refreshWorkspacePanel` replaces them. Only `main.ts` referenced them.
- **Single panel assumption:** `toggleWorkspacePanel`/`refreshWorkspacePanel` locate the panel via `document`/`host` query — the app mounts exactly one. Fine per YAGNI.
- **Version bump / release** are out of scope here (batch into a later tag, e.g. v0.8.22).
