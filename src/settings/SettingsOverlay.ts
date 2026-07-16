// In-app settings panel modeled on WinUI 3 — left sidebar with section
// names, right pane for content. Replaces the old `?` Help button. Holds
// the language picker, keyboard reference, tool reference, yCode syntax
// color editor, and quick-open links for the underlying config files.

import { getVersion } from "@tauri-apps/api/app";

import { t, getLang, setLang, onLangChange, ALL_LANGS, type Lang } from "../i18n/i18n";
import { api } from "../ipc/bridge";
import { pushPopup, popPopup } from "../browser/popupBlur";
import type { YTheme, SettingsSection } from "./types";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";

interface ShortcutEntry {
  keys: string;
  tKey: string;
}

interface ToolEntry {
  cmd: string;
  tKey: string;
}

interface SyntaxField {
  key: keyof YTheme["syntax"];
  tKey: string;
}

const SHORTCUTS: ShortcutEntry[] = [
  { keys: "Ctrl+Alt+1 … 9", tKey: "shortcut.switchWs" },
  { keys: "Ctrl+Shift+H", tKey: "shortcut.splitH" },
  { keys: "Ctrl+Shift+V", tKey: "shortcut.splitV" },
  { keys: "Ctrl+Shift+W", tKey: "shortcut.close" },
  { keys: "Ctrl+Tab", tKey: "shortcut.nextPane" },
  { keys: "Ctrl+Shift+Tab", tKey: "shortcut.prevPane" },
  { keys: "Ctrl+Click (URL)", tKey: "shortcut.openLink" },
  { keys: "Ctrl+Shift+Z", tKey: "shortcut.zoom" },
  { keys: "Ctrl+F", tKey: "shortcut.search" },
  { keys: "Ctrl+Shift+R", tKey: "shortcut.rename" },
  { keys: "Ctrl+Shift+P", tKey: "shortcut.palette" },
  { keys: "Ctrl+Alt+N", tKey: "shortcut.notes" },
  { keys: "Ctrl+V", tKey: "shortcut.paste" },
];

const TOOLS: ToolEntry[] = [
  { cmd: "y", tKey: "help.toolY" },
  { cmd: "ydir", tKey: "help.toolYDir" },
  { cmd: "ymon", tKey: "help.toolYMon" },
  { cmd: "ycode", tKey: "help.toolYCode" },
  { cmd: "ygit", tKey: "help.toolYGit" },
];

const SYNTAX_FIELDS: SyntaxField[] = [
  { key: "keyword", tKey: "settings.syntax.keyword" },
  { key: "string", tKey: "settings.syntax.string" },
  { key: "comment", tKey: "settings.syntax.comment" },
  { key: "number", tKey: "settings.syntax.number" },
  { key: "function", tKey: "settings.syntax.function" },
  { key: "type_name", tKey: "settings.syntax.type_name" },
  { key: "variable", tKey: "settings.syntax.variable" },
  { key: "punctuation", tKey: "settings.syntax.punctuation" },
];

const SECTIONS: { id: SettingsSection; tKey: string; icon: string }[] = [
  { id: "general", tKey: "settings.section.general", icon: "⚙" },
  { id: "syntax", tKey: "settings.section.syntax", icon: "✦" },
  { id: "shortcuts", tKey: "settings.section.shortcuts", icon: "⌨" },
  { id: "tools", tKey: "settings.section.tools", icon: "▤" },
  { id: "config", tKey: "settings.section.config", icon: "📁" },
];

export function mountSettings(parent: HTMLElement, manager: WorkspaceManager): () => void {
  const btn = document.createElement("button");
  btn.type = "button";
  // Match the Ko-fi / GitHub buttons' class + SVG style so the hover
  // border-color animation and sizing line up across the toolbar.
  btn.className = "workspace-bar__icon-btn";
  btn.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`;
  btn.title = t("settings.buttonTitle");
  btn.setAttribute("aria-label", t("settings.buttonTitle"));
  parent.appendChild(btn);

  const backdrop = document.createElement("div");
  backdrop.className = "settings-backdrop";
  document.body.appendChild(backdrop);

  const modal = document.createElement("div");
  modal.className = "settings-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-labelledby", "settings-modal-title");
  document.body.appendChild(modal);

  let isOpen = false;
  let currentSection: SettingsSection = "general";
  let themeDraft: YTheme | null = null;
  let themeDirty = false;

  // Header
  const header = document.createElement("div");
  header.className = "settings-modal__header";
  const title = document.createElement("h2");
  title.id = "settings-modal-title";
  title.className = "settings-modal__title";
  header.appendChild(title);
  const closeBtn = document.createElement("button");
  closeBtn.className = "settings-modal__close";
  closeBtn.textContent = "×";
  closeBtn.title = t("settings.close");
  closeBtn.addEventListener("click", hide);
  header.appendChild(closeBtn);
  modal.appendChild(header);

  // Body: sidebar + content
  const body = document.createElement("div");
  body.className = "settings-modal__body";
  modal.appendChild(body);

  const sidebar = document.createElement("nav");
  sidebar.className = "settings-modal__sidebar";
  body.appendChild(sidebar);

  const content = document.createElement("div");
  content.className = "settings-modal__content";
  body.appendChild(content);

  const sectionButtons = new Map<SettingsSection, HTMLButtonElement>();
  for (const sec of SECTIONS) {
    const sbtn = document.createElement("button");
    sbtn.className = "settings-modal__section-btn";
    sbtn.dataset.section = sec.id;
    sbtn.innerHTML = `<span aria-hidden="true">${sec.icon}</span><span>${t(sec.tKey)}</span>`;
    sbtn.addEventListener("click", () => switchSection(sec.id));
    sidebar.appendChild(sbtn);
    sectionButtons.set(sec.id, sbtn);
  }

  function refreshChrome(): void {
    title.textContent = t("settings.title");
    closeBtn.title = t("settings.close");
    btn.title = t("settings.buttonTitle");
    btn.setAttribute("aria-label", t("settings.buttonTitle"));
    for (const sec of SECTIONS) {
      const b = sectionButtons.get(sec.id);
      if (!b) continue;
      b.innerHTML = `<span aria-hidden="true">${sec.icon}</span><span>${t(sec.tKey)}</span>`;
      b.classList.toggle(
        "settings-modal__section-btn--active",
        sec.id === currentSection,
      );
    }
  }

  function switchSection(id: SettingsSection): void {
    currentSection = id;
    refreshChrome();
    renderCurrentSection();
  }

  function renderCurrentSection(): void {
    content.innerHTML = "";
    const section = document.createElement("section");
    section.className = "settings-section";
    switch (currentSection) {
      case "general":
        renderGeneral(section);
        break;
      case "syntax":
        renderSyntax(section);
        break;
      case "shortcuts":
        renderShortcuts(section);
        break;
      case "tools":
        renderTools(section);
        break;
      case "config":
        renderConfig(section);
        break;
    }
    content.appendChild(section);
  }

  // ── Section: General ────────────────────────────────────────────────
  function renderGeneral(host: HTMLElement): void {
    const h = document.createElement("h3");
    h.textContent = t("settings.general.heading");
    host.appendChild(h);

    const langRow = document.createElement("div");
    langRow.className = "settings-row";
    const label = document.createElement("div");
    label.className = "settings-row__label";
    label.textContent = t("settings.general.language");
    langRow.appendChild(label);
    const sel = document.createElement("select");
    sel.className = "settings-hex-input";
    sel.style.width = "auto";
    const cur = getLang();
    for (const { code, label: ll } of ALL_LANGS) {
      const opt = document.createElement("option");
      opt.value = code;
      opt.textContent = ll;
      if (code === cur) opt.selected = true;
      sel.appendChild(opt);
    }
    sel.addEventListener("change", () => setLang(sel.value as Lang));
    langRow.appendChild(sel);
    const spacer = document.createElement("div");
    langRow.appendChild(spacer);
    host.appendChild(langRow);

    const notifyRow = document.createElement("div");
    notifyRow.className = "settings-row";
    const notifyLabel = document.createElement("div");
    notifyLabel.className = "settings-row__label";
    notifyLabel.textContent = t("settings.general.notifyOnBell");
    notifyRow.appendChild(notifyLabel);
    const notifyToggle = document.createElement("input");
    notifyToggle.type = "checkbox";
    notifyToggle.checked = manager.notifyOnBell;
    notifyToggle.addEventListener("change", () => {
      manager.setNotifyOnBell(notifyToggle.checked);
    });
    notifyRow.appendChild(notifyToggle);
    const notifySpacer = document.createElement("div");
    notifyRow.appendChild(notifySpacer);
    host.appendChild(notifyRow);

    const scrollbackRow = document.createElement("div");
    scrollbackRow.className = "settings-row";
    const scrollbackLabel = document.createElement("div");
    scrollbackLabel.className = "settings-row__label";
    scrollbackLabel.textContent = t("settings.general.persistScrollback");
    scrollbackRow.appendChild(scrollbackLabel);
    const scrollbackToggle = document.createElement("input");
    scrollbackToggle.type = "checkbox";
    scrollbackToggle.checked = manager.persistScrollback;
    scrollbackToggle.addEventListener("change", () => {
      manager.setPersistScrollback(scrollbackToggle.checked);
    });
    scrollbackRow.appendChild(scrollbackToggle);
    const scrollbackSpacer = document.createElement("div");
    scrollbackRow.appendChild(scrollbackSpacer);
    host.appendChild(scrollbackRow);

    const aboutH = document.createElement("h4");
    aboutH.textContent = t("settings.general.about");
    host.appendChild(aboutH);
    const aboutP = document.createElement("p");
    aboutP.textContent = "yMux — A lightweight tmux-inspired terminal multiplexer.";
    host.appendChild(aboutP);

    // Version row — mirrors the language row's layout so it lines up
    // with the rest of the settings list. Tauri's getVersion() reads
    // straight from tauri.conf.json so the value stays in sync with the
    // shipped MSI without any manual wiring on this side.
    const versionRow = document.createElement("div");
    versionRow.className = "settings-row";
    const versionLabel = document.createElement("div");
    versionLabel.className = "settings-row__label";
    versionLabel.textContent = t("settings.general.version");
    versionRow.appendChild(versionLabel);
    const versionValue = document.createElement("div");
    versionValue.style.fontFamily = "Cascadia Mono, Cascadia Code, Consolas, monospace";
    versionValue.style.color = "var(--accent, #7fdbca)";
    versionValue.textContent = "…";
    getVersion().then((v) => {
      versionValue.textContent = `v${v}`;
    });
    versionRow.appendChild(versionValue);
    const versionSpacer = document.createElement("div");
    versionRow.appendChild(versionSpacer);
    host.appendChild(versionRow);
  }

  // ── Section: Syntax Colors ──────────────────────────────────────────
  function renderSyntax(host: HTMLElement): void {
    const h = document.createElement("h3");
    h.textContent = t("settings.syntax.heading");
    host.appendChild(h);
    const desc = document.createElement("p");
    desc.textContent = t("settings.syntax.hint");
    host.appendChild(desc);

    if (!themeDraft) {
      const loading = document.createElement("p");
      loading.textContent = t("settings.syntax.loading");
      host.appendChild(loading);
      return;
    }

    for (const f of SYNTAX_FIELDS) {
      host.appendChild(buildColorRow(f.tKey, f.key));
    }

    const actions = document.createElement("div");
    actions.className = "settings-actions";

    const resetBtn = document.createElement("button");
    resetBtn.className = "settings-btn";
    resetBtn.textContent = t("settings.syntax.reset");
    resetBtn.addEventListener("click", async () => {
      themeDraft = await api.loadSyntaxTheme();
      themeDirty = false;
      renderCurrentSection();
    });
    actions.appendChild(resetBtn);

    const saveBtn = document.createElement("button");
    saveBtn.className = "settings-btn settings-btn--primary";
    saveBtn.textContent = themeDirty
      ? t("settings.syntax.save")
      : t("settings.syntax.saved");
    saveBtn.disabled = !themeDirty;
    saveBtn.addEventListener("click", async () => {
      if (!themeDraft) return;
      try {
        await api.saveSyntaxTheme(themeDraft);
        themeDirty = false;
        renderCurrentSection();
      } catch (e) {
        console.error("saveSyntaxTheme failed:", e);
      }
    });
    actions.appendChild(saveBtn);
    host.appendChild(actions);
  }

  function buildColorRow(labelTKey: string, key: keyof YTheme["syntax"]): HTMLElement {
    const row = document.createElement("div");
    row.className = "settings-row";
    const label = document.createElement("div");
    label.className = "settings-row__label";
    label.textContent = t(labelTKey);
    row.appendChild(label);

    const picker = document.createElement("input");
    picker.type = "color";
    picker.className = "settings-color-picker";
    picker.value = themeDraft!.syntax[key];
    row.appendChild(picker);

    const hex = document.createElement("input");
    hex.type = "text";
    hex.className = "settings-hex-input";
    hex.value = themeDraft!.syntax[key];
    hex.placeholder = "#rrggbb";
    row.appendChild(hex);

    picker.addEventListener("input", () => {
      hex.value = picker.value;
      if (!themeDraft) return;
      themeDraft.syntax[key] = picker.value;
      themeDirty = true;
      // Re-render only the save button enablement; cheap enough to re-render
      // the section to refresh its label.
      const saveBtn = host.parentElement?.querySelector<HTMLButtonElement>(
        ".settings-btn--primary",
      );
      if (saveBtn) {
        saveBtn.disabled = false;
        saveBtn.textContent = t("settings.syntax.save");
      }
    });
    hex.addEventListener("change", () => {
      const v = hex.value.trim();
      if (!/^#[0-9a-fA-F]{6}$/.test(v)) {
        hex.value = themeDraft!.syntax[key];
        return;
      }
      picker.value = v;
      if (!themeDraft) return;
      themeDraft.syntax[key] = v;
      themeDirty = true;
      const saveBtn = host.parentElement?.querySelector<HTMLButtonElement>(
        ".settings-btn--primary",
      );
      if (saveBtn) {
        saveBtn.disabled = false;
        saveBtn.textContent = t("settings.syntax.save");
      }
    });

    // Trick: this row's `host` reference is needed in the listeners above for
    // the saveBtn lookup; we put it on a closure via the wrapper variable.
    const host: HTMLElement = row;

    return row;
  }

  // ── Section: Shortcuts ──────────────────────────────────────────────
  function renderShortcuts(host: HTMLElement): void {
    const h = document.createElement("h3");
    h.textContent = t("settings.shortcuts.heading");
    host.appendChild(h);
    const table = document.createElement("table");
    table.className = "settings-table";
    for (const s of SHORTCUTS) {
      const tr = document.createElement("tr");
      const tdK = document.createElement("td");
      tdK.className = "keys";
      const segs = s.keys.split("+");
      segs.forEach((seg, i) => {
        if (i > 0) tdK.appendChild(document.createTextNode(" + "));
        const kbd = document.createElement("kbd");
        kbd.textContent = seg.trim();
        tdK.appendChild(kbd);
      });
      tr.appendChild(tdK);
      const tdD = document.createElement("td");
      tdD.className = "desc";
      tdD.textContent = t(s.tKey);
      tr.appendChild(tdD);
      table.appendChild(tr);
    }
    host.appendChild(table);
  }

  // ── Section: Tools ──────────────────────────────────────────────────
  function renderTools(host: HTMLElement): void {
    const h = document.createElement("h3");
    h.textContent = t("settings.tools.heading");
    host.appendChild(h);
    const table = document.createElement("table");
    table.className = "settings-table";
    for (const tool of TOOLS) {
      const tr = document.createElement("tr");
      const tdC = document.createElement("td");
      tdC.className = "keys";
      const kbd = document.createElement("kbd");
      kbd.textContent = tool.cmd;
      tdC.appendChild(kbd);
      tr.appendChild(tdC);
      const tdD = document.createElement("td");
      tdD.className = "desc";
      tdD.textContent = t(tool.tKey);
      tr.appendChild(tdD);
      table.appendChild(tr);
    }
    host.appendChild(table);
  }

  // ── Section: Config Files ───────────────────────────────────────────
  function renderConfig(host: HTMLElement): void {
    const h = document.createElement("h3");
    h.textContent = t("settings.config.heading");
    host.appendChild(h);
    const desc = document.createElement("p");
    desc.textContent = t("settings.config.hint");
    host.appendChild(desc);

    const themeRow = document.createElement("div");
    themeRow.className = "settings-row";
    const themeLabel = document.createElement("div");
    themeLabel.className = "settings-row__label";
    themeLabel.textContent = "theme.toml";
    themeRow.appendChild(themeLabel);
    const themeBtn = document.createElement("button");
    themeBtn.className = "settings-btn";
    themeBtn.textContent = t("settings.config.open");
    themeBtn.addEventListener("click", () =>
      void api.openConfigPath("theme").catch((e) => console.error(e)),
    );
    themeRow.appendChild(themeBtn);
    const themeSpacer = document.createElement("div");
    themeRow.appendChild(themeSpacer);
    host.appendChild(themeRow);

    const dirRow = document.createElement("div");
    dirRow.className = "settings-row";
    const dirLabel = document.createElement("div");
    dirLabel.className = "settings-row__label";
    dirLabel.textContent = t("settings.config.folder");
    dirRow.appendChild(dirLabel);
    const dirBtn = document.createElement("button");
    dirBtn.className = "settings-btn";
    dirBtn.textContent = t("settings.config.openFolder");
    dirBtn.addEventListener("click", () =>
      void api.openConfigPath("folder").catch((e) => console.error(e)),
    );
    dirRow.appendChild(dirBtn);
    const dirSpacer = document.createElement("div");
    dirRow.appendChild(dirSpacer);
    host.appendChild(dirRow);
  }

  // ── show / hide ─────────────────────────────────────────────────────
  async function show(): Promise<void> {
    if (isOpen) return;
    isOpen = true;
    pushPopup();
    backdrop.classList.add("settings-backdrop--visible");
    modal.classList.add("settings-modal--visible");
    backdrop.setAttribute("aria-hidden", "false");
    refreshChrome();
    renderCurrentSection();
    // Lazy-load the syntax theme so the editor is populated when the
    // user clicks the Syntax Colors tab.
    if (!themeDraft) {
      try {
        themeDraft = await api.loadSyntaxTheme();
        themeDirty = false;
        if (currentSection === "syntax") renderCurrentSection();
      } catch (e) {
        console.error("loadSyntaxTheme failed:", e);
      }
    }
  }

  function hide(): void {
    if (!isOpen) return;
    isOpen = false;
    popPopup();
    backdrop.classList.remove("settings-backdrop--visible");
    modal.classList.remove("settings-modal--visible");
    backdrop.setAttribute("aria-hidden", "true");
    btn.focus();
  }

  btn.addEventListener("click", () => void show());
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", onKey);
  function onKey(ev: KeyboardEvent): void {
    if (ev.key === "Escape" && isOpen) {
      ev.preventDefault();
      hide();
    }
  }

  const cleanupLang = onLangChange(() => {
    if (isOpen) {
      refreshChrome();
      renderCurrentSection();
    }
  });

  return () => {
    cleanupLang();
    document.removeEventListener("keydown", onKey);
    btn.remove();
    backdrop.remove();
    modal.remove();
  };
}
