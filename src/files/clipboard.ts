// The files clipboard: one per app, shared by every files pane and the dock,
// so "copy in the dock, paste in a pane" works. Holds several items (ydir's
// held exactly one). Paths only — the OS clipboard gets the same paths as
// text on copy, so they paste into a terminal too.

export interface ClipItem {
  path: string;
  name: string;
  is_dir: boolean;
}

export interface FilesClipboard {
  mode: "copy" | "cut";
  /// Id of the pane that made it. Esc in that pane cancels a cut; Esc in
  /// any other pane leaves it alone.
  owner: string;
  /// The directory the items were taken from, as the pane held it. Used to
  /// spot a paste back into the same folder; the backend stays authoritative.
  dir: string;
  items: ClipItem[];
}

let current: FilesClipboard | null = null;
const listeners = new Set<() => void>();

export function getClipboard(): FilesClipboard | null {
  return current;
}

export function setClipboard(next: FilesClipboard | null): void {
  current = next;
  for (const cb of listeners) cb();
}

/// Subscribe to changes (a cut dims its rows in every pane showing them).
export function onClipboardChange(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
