import { t, getLang, setLang, onLangChange, ALL_LANGS, type Lang } from "../i18n/i18n";

interface ShortcutEntry {
  keys: string;
  tKey: string;
}

const SHORTCUTS: ShortcutEntry[] = [
  { keys: "Ctrl+Alt+1 … 9", tKey: "shortcut.switchWs" },
  { keys: "Ctrl+Shift+H",   tKey: "shortcut.splitH" },
  { keys: "Ctrl+Shift+V",   tKey: "shortcut.splitV" },
  { keys: "Ctrl+Shift+W",   tKey: "shortcut.close" },
  { keys: "Ctrl+Tab",       tKey: "shortcut.nextPane" },
  { keys: "Ctrl+Shift+Tab", tKey: "shortcut.prevPane" },
  { keys: "Ctrl+Click (URL)", tKey: "shortcut.openLink" },
  { keys: "Ctrl+Shift+Z",   tKey: "shortcut.zoom" },
  { keys: "Ctrl+F",         tKey: "shortcut.search" },
  { keys: "Ctrl+Shift+R",   tKey: "shortcut.rename" },
];

export function mountHelpButton(parent: HTMLElement): () => void {
  const btn = document.createElement("button");
  btn.className = "workspace-bar__help";
  btn.textContent = "?";
  btn.title = t("help.buttonTitle");
  btn.setAttribute("aria-label", t("help.buttonTitle"));
  parent.appendChild(btn);

  const backdrop = document.createElement("div");
  backdrop.className = "help-backdrop";
  backdrop.setAttribute("aria-hidden", "true");
  document.body.appendChild(backdrop);

  const modal = document.createElement("div");
  modal.className = "help-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-labelledby", "help-modal-title");
  document.body.appendChild(modal);

  function render() {
    modal.innerHTML = "";

    const header = document.createElement("div");
    header.className = "help-modal__header";

    const title = document.createElement("h2");
    title.id = "help-modal-title";
    title.className = "help-modal__title";
    title.textContent = t("help.title");
    header.appendChild(title);

    const langWrap = document.createElement("div");
    langWrap.className = "help-modal__lang-wrap";

    const langLabel = document.createElement("label");
    langLabel.className = "help-modal__lang-label";
    langLabel.textContent = t("help.langLabel") + ":";

    const sel = document.createElement("select");
    sel.className = "help-modal__lang-sel";
    const cur = getLang();
    for (const { code, label } of ALL_LANGS) {
      const opt = document.createElement("option");
      opt.value = code;
      opt.textContent = label;
      if (code === cur) opt.selected = true;
      sel.appendChild(opt);
    }
    sel.addEventListener("change", () => {
      setLang(sel.value as Lang);
    });

    langWrap.appendChild(langLabel);
    langWrap.appendChild(sel);
    header.appendChild(langWrap);
    modal.appendChild(header);

    const table = document.createElement("table");
    table.className = "help-modal__table";

    for (const s of SHORTCUTS) {
      const tr = document.createElement("tr");

      const tdKeys = document.createElement("td");
      tdKeys.className = "help-modal__keys";
      const segments = s.keys.split("+");
      segments.forEach((seg, i) => {
        if (i > 0) {
          tdKeys.appendChild(document.createTextNode(" + "));
        }
        const kbd = document.createElement("kbd");
        kbd.textContent = seg.trim();
        tdKeys.appendChild(kbd);
      });

      const tdDesc = document.createElement("td");
      tdDesc.className = "help-modal__desc";
      tdDesc.textContent = t(s.tKey);

      tr.appendChild(tdKeys);
      tr.appendChild(tdDesc);
      table.appendChild(tr);
    }

    modal.appendChild(table);

    const closeBtn = document.createElement("button");
    closeBtn.className = "help-modal__close";
    closeBtn.textContent = t("help.close");
    closeBtn.addEventListener("click", hide);
    modal.appendChild(closeBtn);
  }

  function show() {
    render();
    backdrop.classList.add("help-backdrop--visible");
    modal.classList.add("help-modal--visible");
    backdrop.setAttribute("aria-hidden", "false");
    const firstFocusable = modal.querySelector<HTMLElement>(
      "button, select, [tabindex]",
    );
    firstFocusable?.focus();
  }

  function hide() {
    backdrop.classList.remove("help-backdrop--visible");
    modal.classList.remove("help-modal--visible");
    backdrop.setAttribute("aria-hidden", "true");
    btn.focus();
  }

  btn.addEventListener("click", show);

  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && modal.classList.contains("help-modal--visible")) {
      ev.preventDefault();
      hide();
    }
  });

  const cleanupLang = onLangChange(() => {
    btn.title = t("help.buttonTitle");
    btn.setAttribute("aria-label", t("help.buttonTitle"));
    if (modal.classList.contains("help-modal--visible")) render();
  });

  return () => {
    cleanupLang();
    btn.remove();
    backdrop.remove();
    modal.remove();
  };
}
