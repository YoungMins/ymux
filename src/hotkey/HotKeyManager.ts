import type { HotKeyDef } from "../types";
import { t } from "../i18n/i18n";

const COLOR_PRESETS = [
  "#0b0f14", "#1a1a2e", "#16213e", "#0f3460",
  "#1b1b2f", "#162447", "#1f4068", "#1a3c40",
  "#2d132c", "#3b0a30", "#461220", "#1c0c1b",
  "#0a1628", "#0d1b2a", "#1b263b", "#1d3557",
];

export function openHotKeyManager(
  initial: HotKeyDef[],
  initialBgColor: string | null,
  onCommit: (next: HotKeyDef[]) => void,
  onBgColorChange: (color: string | null) => void,
): void {
  let draft: HotKeyDef[] = initial.map((h) => ({ ...h }));

  const backdrop = document.createElement("div");
  backdrop.className = "help-backdrop";

  const modal = document.createElement("div");
  modal.className = "help-modal hotkey-modal";

  // ── Background Color Section ──
  const colorTitle = document.createElement("h2");
  colorTitle.textContent = t("hotkey.bgColor") || "Background Color";
  modal.appendChild(colorTitle);

  const colorRow = document.createElement("div");
  colorRow.style.display = "flex";
  colorRow.style.gap = "6px";
  colorRow.style.flexWrap = "wrap";
  colorRow.style.alignItems = "center";
  colorRow.style.marginBottom = "12px";

  for (const preset of COLOR_PRESETS) {
    const swatch = document.createElement("button");
    swatch.type = "button";
    swatch.style.width = "22px";
    swatch.style.height = "22px";
    swatch.style.background = preset;
    swatch.style.border = preset === (initialBgColor || "#0b0f14")
      ? "2px solid #7fdbca"
      : "1px solid #1e2a38";
    swatch.style.borderRadius = "4px";
    swatch.style.cursor = "pointer";
    swatch.style.padding = "0";
    swatch.title = preset;
    swatch.addEventListener("click", () => {
      onBgColorChange(preset === "#0b0f14" ? null : preset);
      // Update border highlights
      for (const ch of colorRow.querySelectorAll("button")) {
        (ch as HTMLElement).style.border = ch === swatch
          ? "2px solid #7fdbca"
          : "1px solid #1e2a38";
      }
      // Update custom input
      const inp = colorRow.querySelector("input");
      if (inp) inp.value = preset;
    });
    colorRow.appendChild(swatch);
  }

  const customInput = document.createElement("input");
  customInput.type = "text";
  customInput.className = "hotkey-modal__label-input";
  customInput.value = initialBgColor || "#0b0f14";
  customInput.placeholder = "#hex";
  customInput.style.width = "80px";
  customInput.addEventListener("change", () => {
    const v = customInput.value.trim();
    if (/^#[0-9a-fA-F]{6}$/.test(v)) {
      onBgColorChange(v === "#0b0f14" ? null : v);
    }
  });
  colorRow.appendChild(customInput);

  modal.appendChild(colorRow);

  // ── HotKey Section ──
  const title = document.createElement("h2");
  title.textContent = t("hotkey.title");
  modal.appendChild(title);

  const hint = document.createElement("p");
  hint.className = "hotkey-modal__hint";
  hint.textContent = t("hotkey.hint");
  modal.appendChild(hint);

  const list = document.createElement("div");
  list.className = "hotkey-modal__list";
  modal.appendChild(list);

  const footer = document.createElement("div");
  footer.className = "hotkey-modal__footer";
  modal.appendChild(footer);

  const addBtn = document.createElement("button");
  addBtn.type = "button";
  addBtn.className = "hotkey-modal__btn";
  addBtn.textContent = t("hotkey.add");
  addBtn.addEventListener("click", () => {
    draft.push({ label: "", command: "", batch: false });
    renderList();
  });
  footer.appendChild(addBtn);

  const spacer = document.createElement("div");
  spacer.style.flex = "1";
  footer.appendChild(spacer);

  const cancelBtn = document.createElement("button");
  cancelBtn.type = "button";
  cancelBtn.className = "hotkey-modal__btn";
  cancelBtn.textContent = t("hotkey.cancel");
  cancelBtn.addEventListener("click", close);
  footer.appendChild(cancelBtn);

  const saveBtn = document.createElement("button");
  saveBtn.type = "button";
  saveBtn.className = "hotkey-modal__btn hotkey-modal__btn--primary";
  saveBtn.textContent = t("hotkey.save");
  saveBtn.addEventListener("click", () => {
    const cleaned = draft
      .map((h) => ({
        label: h.label.trim(),
        command: h.command,
        batch: !!h.batch,
      }))
      .filter((h) => h.command.trim().length > 0);
    onCommit(cleaned);
    close();
  });
  footer.appendChild(saveBtn);

  backdrop.appendChild(modal);
  document.body.appendChild(backdrop);
  backdrop.classList.add("help-backdrop--visible");
  modal.classList.add("help-modal--visible");

  backdrop.addEventListener("click", (ev) => {
    if (ev.target === backdrop) close();
  });
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") close();
  };
  window.addEventListener("keydown", onKey);

  function close(): void {
    window.removeEventListener("keydown", onKey);
    backdrop.remove();
  }

  function renderList(): void {
    while (list.firstChild) list.removeChild(list.firstChild);
    if (draft.length === 0) {
      const empty = document.createElement("p");
      empty.className = "hotkey-modal__empty";
      empty.textContent = t("hotkey.empty");
      list.appendChild(empty);
      return;
    }
    draft.forEach((def, idx) => {
      list.appendChild(renderRow(def, idx));
    });
  }

  function renderRow(def: HotKeyDef, idx: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "hotkey-modal__row";

    const labelInput = document.createElement("input");
    labelInput.type = "text";
    labelInput.placeholder = t("hotkey.labelPlaceholder");
    labelInput.value = def.label;
    labelInput.className = "hotkey-modal__label-input";
    labelInput.addEventListener("input", () => {
      draft[idx].label = labelInput.value;
    });

    const cmdInput = document.createElement("textarea");
    cmdInput.placeholder = t("hotkey.commandPlaceholder");
    cmdInput.value = def.command;
    cmdInput.rows = 2;
    cmdInput.className = "hotkey-modal__cmd-input";
    cmdInput.addEventListener("input", () => {
      draft[idx].command = cmdInput.value;
    });

    const batchLabel = document.createElement("label");
    batchLabel.className = "hotkey-modal__batch";
    const batchInput = document.createElement("input");
    batchInput.type = "checkbox";
    batchInput.checked = !!def.batch;
    batchInput.addEventListener("change", () => {
      draft[idx].batch = batchInput.checked;
    });
    batchLabel.appendChild(batchInput);
    batchLabel.appendChild(document.createTextNode(` ${t("hotkey.batch")}`));

    const controls = document.createElement("div");
    controls.className = "hotkey-modal__controls";

    const upBtn = rowBtn("↑", t("hotkey.moveUp"), () => {
      if (idx === 0) return;
      [draft[idx - 1], draft[idx]] = [draft[idx], draft[idx - 1]];
      renderList();
    });
    const downBtn = rowBtn("↓", t("hotkey.moveDown"), () => {
      if (idx === draft.length - 1) return;
      [draft[idx + 1], draft[idx]] = [draft[idx], draft[idx + 1]];
      renderList();
    });
    const delBtn = rowBtn("✕", t("hotkey.delete"), () => {
      draft.splice(idx, 1);
      renderList();
    });
    controls.appendChild(upBtn);
    controls.appendChild(downBtn);
    controls.appendChild(delBtn);

    row.appendChild(labelInput);
    row.appendChild(cmdInput);
    row.appendChild(batchLabel);
    row.appendChild(controls);
    return row;
  }

  renderList();
}

function rowBtn(
  icon: string,
  title: string,
  onClick: () => void,
): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "hotkey-modal__row-btn";
  b.textContent = icon;
  b.title = title;
  b.addEventListener("click", (ev) => {
    ev.preventDefault();
    onClick();
  });
  return b;
}
