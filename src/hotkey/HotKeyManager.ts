// Modal for CRUD on a pane's HotKey list. Invoked by the `⚙` button in
// `HotKeyBar`. Reuses the same overlay pattern as `HelpOverlay` (backdrop +
// centered panel, Esc / outside-click to dismiss) so the visual language
// stays consistent.

import type { HotKeyDef } from "../types";

export function openHotKeyManager(
  initial: HotKeyDef[],
  onCommit: (next: HotKeyDef[]) => void,
): void {
  let draft: HotKeyDef[] = initial.map((h) => ({ ...h }));

  const backdrop = document.createElement("div");
  backdrop.className = "help-backdrop";

  const modal = document.createElement("div");
  modal.className = "help-modal hotkey-modal";

  const title = document.createElement("h2");
  title.textContent = "Manage HotKeys";
  modal.appendChild(title);

  const hint = document.createElement("p");
  hint.className = "hotkey-modal__hint";
  hint.textContent =
    "배치 모드: 여러 줄 명령을 한 줄씩 순차 실행합니다. 비활성화 시 전체를 한 번에 전송합니다.";
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
  addBtn.textContent = "+ Add HotKey";
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
  cancelBtn.textContent = "Cancel";
  cancelBtn.addEventListener("click", close);
  footer.appendChild(cancelBtn);

  const saveBtn = document.createElement("button");
  saveBtn.type = "button";
  saveBtn.className = "hotkey-modal__btn hotkey-modal__btn--primary";
  saveBtn.textContent = "Save";
  saveBtn.addEventListener("click", () => {
    // Drop empty rows silently — users will often add then abandon a slot.
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
      empty.textContent = "아직 등록된 HotKey가 없습니다.";
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
    labelInput.placeholder = "Label (e.g. pull)";
    labelInput.value = def.label;
    labelInput.className = "hotkey-modal__label-input";
    labelInput.addEventListener("input", () => {
      draft[idx].label = labelInput.value;
    });

    const cmdInput = document.createElement("textarea");
    cmdInput.placeholder = "Command — newline = next line in batch mode";
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
    batchLabel.appendChild(document.createTextNode(" batch"));

    const controls = document.createElement("div");
    controls.className = "hotkey-modal__controls";

    const upBtn = rowBtn("↑", "Move up", () => {
      if (idx === 0) return;
      [draft[idx - 1], draft[idx]] = [draft[idx], draft[idx - 1]];
      renderList();
    });
    const downBtn = rowBtn("↓", "Move down", () => {
      if (idx === draft.length - 1) return;
      [draft[idx + 1], draft[idx]] = [draft[idx], draft[idx + 1]];
      renderList();
    });
    const delBtn = rowBtn("✕", "Delete", () => {
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
