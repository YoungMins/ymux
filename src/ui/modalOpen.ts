// "A modal overlay is open": Notes, Settings, the Command Palette, the
// HotKeyManager modal, or any Dialog.ts dialog (rename, confirm, switch
// account, keep lane, queue prompt ...). Dialogs also stop their own keydowns
// from inside the card, but a key pressed while focus sits outside it would
// reach the window handler, so the backdrop counts too.
// main.ts's global keydown handler stands down while one is up, because its
// shortcuts (split, close, new tab, workspace switch) act on the panes behind
// the overlay. Read from the overlays' visibility classes, which each sets and
// clears on show/hide, so no overlay needs a new export.

const OPEN_SELECTORS = [
  ".notes-modal--visible",
  ".settings-modal--visible",
  ".palette--visible",
  ".help-modal--visible",
  ".dialog-backdrop",
  ".queue-dialog",
].join(",");

export function isModalOverlayOpen(root: ParentNode = document): boolean {
  return root.querySelector(OPEN_SELECTORS) !== null;
}
