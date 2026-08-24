import { type CommandDef, fuzzyMatch } from "./commands";
import { t, onLangChange } from "../i18n/i18n";
import { pushPopup, popPopup } from "../browser/popupBlur";
import { shortcutLabel } from "../platform";

let paletteEl: HTMLElement | null = null;
let backdropEl: HTMLElement | null = null;
let inputEl: HTMLInputElement | null = null;
let listEl: HTMLElement | null = null;
let commands: CommandDef[] = [];
let filtered: CommandDef[] = [];
let selectedIdx = 0;
let visible = false;

export function mountCommandPalette(
  container: HTMLElement,
  cmds: CommandDef[],
): void {
  commands = cmds;

  backdropEl = document.createElement("div");
  backdropEl.className = "palette-backdrop";
  backdropEl.addEventListener("click", hide);

  paletteEl = document.createElement("div");
  paletteEl.className = "palette";

  const header = document.createElement("div");
  header.className = "palette__header";

  inputEl = document.createElement("input");
  inputEl.className = "palette__input";
  inputEl.type = "text";
  inputEl.addEventListener("input", onInput);
  inputEl.addEventListener("keydown", onKeyDown);
  header.appendChild(inputEl);

  listEl = document.createElement("div");
  listEl.className = "palette__list";

  paletteEl.appendChild(header);
  paletteEl.appendChild(listEl);
  container.appendChild(backdropEl);
  container.appendChild(paletteEl);

  onLangChange(() => updatePlaceholder());
  updatePlaceholder();
}

function updatePlaceholder(): void {
  if (inputEl) {
    inputEl.placeholder = t("palette.placeholder") || "Type a command…";
  }
}

export function toggle(): void {
  if (visible) hide();
  else show();
}

function show(): void {
  if (!paletteEl || !backdropEl || !inputEl) return;
  if (visible) return;
  visible = true;
  pushPopup();
  backdropEl.classList.add("palette-backdrop--visible");
  paletteEl.classList.add("palette--visible");
  inputEl.value = "";
  selectedIdx = 0;
  filtered = [...commands];
  renderList();
  inputEl.focus();
}

function hide(): void {
  if (!paletteEl || !backdropEl) return;
  if (!visible) return;
  visible = false;
  popPopup();
  backdropEl.classList.remove("palette-backdrop--visible");
  paletteEl.classList.remove("palette--visible");
}

function onInput(): void {
  if (!inputEl) return;
  const query = inputEl.value.trim();
  if (query === "") {
    filtered = [...commands];
  } else {
    filtered = commands.filter((cmd) => {
      const label = cmd.label();
      const kb = shortcutLabel(cmd.keybinding ?? "");
      return fuzzyMatch(query, label) || fuzzyMatch(query, kb) || fuzzyMatch(query, cmd.id);
    });
  }
  selectedIdx = 0;
  renderList();
}

function onKeyDown(ev: KeyboardEvent): void {
  if (ev.key === "Escape") {
    ev.preventDefault();
    hide();
  } else if (ev.key === "ArrowDown") {
    ev.preventDefault();
    selectedIdx = Math.min(selectedIdx + 1, filtered.length - 1);
    renderList();
  } else if (ev.key === "ArrowUp") {
    ev.preventDefault();
    selectedIdx = Math.max(selectedIdx - 1, 0);
    renderList();
  } else if (ev.key === "Enter") {
    ev.preventDefault();
    if (filtered[selectedIdx]) {
      hide();
      void filtered[selectedIdx].action();
    }
  }
}

function renderList(): void {
  if (!listEl) return;
  listEl.innerHTML = "";

  const max = Math.min(filtered.length, 12);
  for (let i = 0; i < max; i++) {
    const cmd = filtered[i];
    const row = document.createElement("div");
    row.className = "palette__item";
    if (i === selectedIdx) row.classList.add("palette__item--selected");

    const label = document.createElement("span");
    label.className = "palette__label";
    label.textContent = cmd.label();

    row.appendChild(label);

    if (cmd.keybinding) {
      const kb = document.createElement("span");
      kb.className = "palette__keybinding";
      kb.textContent = shortcutLabel(cmd.keybinding);
      row.appendChild(kb);
    }

    row.addEventListener("click", () => {
      hide();
      void cmd.action();
    });
    row.addEventListener("mouseenter", () => {
      selectedIdx = i;
      renderList();
    });

    listEl.appendChild(row);
  }

  if (filtered.length === 0) {
    const empty = document.createElement("div");
    empty.className = "palette__empty";
    empty.textContent = t("palette.empty") || "No matching commands";
    listEl.appendChild(empty);
  }
}
