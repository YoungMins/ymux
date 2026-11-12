// Small badges laid over the top-right corner of a pane element: how many
// prompts are queued for it and whether broadcast input reaches it. One
// overlay on the `.pane` element serves standalone panes and tab children
// alike (the title row of a tab group is shared and repainted elsewhere).

export interface PaneFlags {
  queuedText: string | null;
  broadcastText: string | null;
}

/// Write the flags into `el`: a `.pane-flags` overlay holding one chip per
/// flag, and `pane--broadcast` for the outline. Idempotent; the overlay is
/// removed when there is nothing to show.
export function paintPaneFlags(el: HTMLElement, flags: PaneFlags): void {
  el.classList.toggle("pane--broadcast", flags.broadcastText !== null);
  let box = el.querySelector<HTMLElement>(":scope > .pane-flags");
  if (flags.queuedText === null && flags.broadcastText === null) {
    box?.remove();
    return;
  }
  if (!box) {
    box = document.createElement("div");
    box.className = "pane-flags";
    el.appendChild(box);
  }
  sync(box, "pane-flag--queue", flags.queuedText);
  sync(box, "pane-flag--broadcast", flags.broadcastText);
}

function sync(box: HTMLElement, cls: string, text: string | null): void {
  let chip = box.querySelector<HTMLElement>(`:scope > .${cls}`);
  if (text === null) {
    chip?.remove();
    return;
  }
  if (!chip) {
    chip = document.createElement("span");
    chip.className = `pane-flag ${cls}`;
    box.appendChild(chip);
  }
  if (chip.textContent !== text) chip.textContent = text;
}
