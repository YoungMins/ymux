// Pure model of the files pane: ordering, the hidden rule, the selection
// state machine, overwrite resolution and the path-string helpers. No DOM,
// no IPC, so every rule here runs under vitest.
//
// A note on names vs paths (CLAUDE.md rule 15). Selection is keyed by *name
// within one listing*, never by path: every name in a listing comes from the
// same `read_dir`, so two names that differ at all are two entries. Where this
// module does compare names loosely (`findConflict`, `uniqueName`,
// `typeAhead`), it errs towards "same" — lower-case plus NFC — because the
// cost of a false match is an extra prompt or a skipped number, while a false
// miss is a clobbered file on a case-insensitive volume. The backend stays
// authoritative for real path identity (`fsx::same_file`).

/// One listing row. Mirrors `src-tauri/src/fsx.rs::DirEntryInfo`.
export interface FileEntry {
  name: string;
  /// Absolute path, raw spelling. Open it, never compare it.
  path: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  modified_ms: number;
}

// ── Ordering and the hidden rule ────────────────────────────────────────────

/// Code point comparison. JS `<` compares UTF-16 code units, which puts an
/// astral character (a surrogate pair, 0xD800…) before U+E000–U+FFFF; Rust's
/// `String` `Ord` is byte order, i.e. code point order. Matching Rust keeps
/// the listing in exactly the order `fsx::sort_entries` produced.
function compareCodePoints(a: string, b: string): number {
  const ia = a[Symbol.iterator]();
  const ib = b[Symbol.iterator]();
  for (;;) {
    const x = ia.next();
    const y = ib.next();
    if (x.done) return y.done ? 0 : -1;
    if (y.done) return 1;
    const cx = x.value.codePointAt(0)!;
    const cy = y.value.codePointAt(0)!;
    if (cx !== cy) return cx < cy ? -1 : 1;
  }
}

/// Directories first, then by lower-cased name — `fsx::sort_entries`'s rule,
/// which is `tools/ydir`'s, so the pane lists in the order users already
/// have. Returns a new array.
export function sortEntries(entries: readonly FileEntry[]): FileEntry[] {
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return compareCodePoints(a.name.toLowerCase(), b.name.toLowerCase());
  });
}

export function isHiddenName(name: string): boolean {
  return name.startsWith(".") && name !== "." && name !== "..";
}

/// The dotfile rule. The Windows hidden *attribute* needs a syscall, so the
/// backend applies that half in `fs_list_dir`.
export function applyHidden(entries: readonly FileEntry[], showHidden: boolean): FileEntry[] {
  return showHidden ? [...entries] : entries.filter((e) => !isHiddenName(e.name));
}

// ── Selection ───────────────────────────────────────────────────────────────

/// `cursor` is the keyboard row; `anchor` is where a Shift-range starts;
/// `selected` holds names. Indices are into the current `names` list.
export interface Selection {
  cursor: number;
  anchor: number;
  selected: ReadonlySet<string>;
}

function clamp(i: number, len: number): number {
  if (len <= 0) return 0;
  return Math.min(len - 1, Math.max(0, i));
}

export function selectOnly(names: readonly string[], index: number): Selection {
  const i = clamp(index, names.length);
  return {
    cursor: i,
    anchor: i,
    selected: names.length ? new Set([names[i]]) : new Set(),
  };
}

/// Shift-click / Shift-arrow: the anchor..index range replaces the selection.
export function extendTo(sel: Selection, names: readonly string[], index: number): Selection {
  const i = clamp(index, names.length);
  const anchor = clamp(sel.anchor, names.length);
  const [lo, hi] = anchor <= i ? [anchor, i] : [i, anchor];
  return { cursor: i, anchor, selected: new Set(names.slice(lo, hi + 1)) };
}

/// Ctrl-click: flip one row and move the anchor there.
export function toggleAt(sel: Selection, names: readonly string[], index: number): Selection {
  const i = clamp(index, names.length);
  const name = names[i];
  const next = new Set(sel.selected);
  if (name !== undefined) {
    if (next.has(name)) next.delete(name);
    else next.add(name);
  }
  return { cursor: i, anchor: i, selected: next };
}

export function moveCursor(
  sel: Selection,
  names: readonly string[],
  delta: number,
  extend: boolean,
): Selection {
  const i = clamp(sel.cursor + delta, names.length);
  return extend ? extendTo(sel, names, i) : selectOnly(names, i);
}

export function selectAll(sel: Selection, names: readonly string[]): Selection {
  return { cursor: sel.cursor, anchor: sel.anchor, selected: new Set(names) };
}

/// The names an action applies to, in list order: the selection, or the
/// cursor row when nothing is selected.
export function targetNames(sel: Selection, names: readonly string[]): string[] {
  const picked = names.filter((n) => sel.selected.has(n));
  if (picked.length) return picked;
  const at = names[sel.cursor];
  return at === undefined ? [] : [at];
}

/// Carry a selection across a re-listing (refresh, a paste, a rename). Names
/// that survived stay selected; the cursor follows its name, or `prefer`
/// (e.g. the directory just left by "up"), or stays at its index.
export function reconcile(
  sel: Selection,
  prevNames: readonly string[],
  nextNames: readonly string[],
  prefer?: string,
): Selection {
  const present = new Set(nextNames);
  const selected = new Set([...sel.selected].filter((n) => present.has(n)));
  const byPrefer = prefer !== undefined ? nextNames.indexOf(prefer) : -1;
  if (byPrefer >= 0) {
    return { cursor: byPrefer, anchor: byPrefer, selected: new Set([prefer!]) };
  }
  const cursorName = prevNames[sel.cursor];
  const followed = cursorName !== undefined ? nextNames.indexOf(cursorName) : -1;
  const cursor = followed >= 0 ? followed : clamp(sel.cursor, nextNames.length);
  const anchorName = prevNames[sel.anchor];
  const anchorAt = anchorName !== undefined ? nextNames.indexOf(anchorName) : -1;
  return { cursor, anchor: anchorAt >= 0 ? anchorAt : cursor, selected };
}

/// After deleting `deleted`, the row the cursor should land on: the first
/// survivor at or after the cursor, else the last one before it.
export function nextSelectionAfterDelete(
  names: readonly string[],
  deleted: ReadonlySet<string>,
  cursor: number,
): string | null {
  for (let i = cursor; i < names.length; i++) if (!deleted.has(names[i])) return names[i];
  for (let i = Math.min(cursor, names.length) - 1; i >= 0; i--) {
    if (!deleted.has(names[i])) return names[i];
  }
  return null;
}

// ── Overwrite resolution ────────────────────────────────────────────────────

export type OverwriteChoice = "replace" | "keep-both" | "skip";

export type WritePlan = { action: "write"; name: string; overwrite: boolean } | { action: "skip" };

function looseKey(name: string): string {
  return name.normalize("NFC").toLowerCase();
}

/// The entry `name` would collide with in `entries`, if any.
export function findConflict(name: string, entries: readonly FileEntry[]): FileEntry | undefined {
  const k = looseKey(name);
  return entries.find((e) => looseKey(e.name) === k);
}

/// Where the " (n)" goes: before the last extension of a file; at the end
/// of a directory or a dotfile with no other dot.
function splitExt(name: string, isDir: boolean): [string, string] {
  const end = stemEnd(name, isDir);
  return [name.slice(0, end), name.slice(end)];
}

/// `foo.txt` → `foo (2).txt`, then `(3)`, … skipping any number taken.
export function uniqueName(name: string, isDir: boolean, taken: Iterable<string>): string {
  const used = new Set([...taken].map(looseKey));
  if (!used.has(looseKey(name))) return name;
  const [stem, ext] = splitExt(name, isDir);
  for (let n = 2; ; n++) {
    const candidate = `${stem} (${n})${ext}`;
    if (!used.has(looseKey(candidate))) return candidate;
  }
}

/// Only a file may replace a file. Replacing a directory would have to
/// either merge (surprising) or delete the old tree first (destructive, and
/// `fs_move` cannot do it atomically), so the pane refuses and offers "keep
/// both" instead.
export function canReplace(srcIsDir: boolean, dest: FileEntry | undefined): boolean {
  return !!dest && !srcIsDir && !dest.is_dir;
}

export function resolveOverwrite(
  name: string,
  srcIsDir: boolean,
  dest: FileEntry | undefined,
  choice: OverwriteChoice,
  taken: Iterable<string>,
): WritePlan {
  if (!dest) return { action: "write", name, overwrite: false };
  if (choice === "skip") return { action: "skip" };
  if (choice === "replace" && canReplace(srcIsDir, dest)) {
    return { action: "write", name, overwrite: true };
  }
  return { action: "write", name: uniqueName(name, srcIsDir, taken), overwrite: false };
}

// ── Path strings ────────────────────────────────────────────────────────────
//
// String surgery on one path the pane already holds — never equality between
// two paths (rule 15). Windows syntax is recognised by its shape (drive
// letter or `\\` prefix), not by the platform ymux runs on, because a Windows
// ymux also shows WSL (`\\wsl.localhost\…`) and a macOS one only POSIX.

function isWindowsPath(p: string): boolean {
  return /^[A-Za-z]:([\\/]|$)/.test(p) || p.startsWith("\\\\");
}

function sepOf(p: string): string {
  return isWindowsPath(p) ? "\\" : "/";
}

/// Length of the root prefix: `C:\`, `\\server\share\`, `\\?\C:\`, `/`.
function rootLength(p: string): number {
  if (p.startsWith("\\\\?\\") || p.startsWith("\\\\.\\")) {
    const m = /^\\\\[?.]\\[A-Za-z]:\\?/.exec(p);
    if (m) return m[0].length;
  }
  if (p.startsWith("\\\\")) {
    const m = /^\\\\[^\\]+\\[^\\]+\\?/.exec(p);
    return m ? m[0].length : p.length;
  }
  const drive = /^[A-Za-z]:[\\/]?/.exec(p);
  if (drive) return drive[0].length;
  return p.startsWith("/") ? 1 : 0;
}

/// Root with its trailing separator, and the rest split into components.
function splitPath(p: string): { root: string; parts: string[] } {
  const n = rootLength(p);
  let root = p.slice(0, n);
  const sep = sepOf(p);
  if (root && !root.endsWith("\\") && !root.endsWith("/")) root += sep;
  const parts = p
    .slice(n)
    .split(/[\\/]/)
    .filter((s) => s.length > 0);
  return { root, parts };
}

export function joinPath(dir: string, name: string): string {
  const sep = sepOf(dir);
  return dir.endsWith("\\") || dir.endsWith("/") ? dir + name : dir + sep + name;
}

export function parentPath(p: string): string | null {
  const { root, parts } = splitPath(p);
  if (parts.length === 0) return null;
  if (parts.length === 1) return root || null;
  return root + parts.slice(0, -1).join(sepOf(p));
}

export function baseName(p: string): string {
  const { root, parts } = splitPath(p);
  if (parts.length) return parts[parts.length - 1];
  return rootLabel(root);
}

function rootLabel(root: string): string {
  if (root === "/") return "/";
  return root.replace(/[\\/]+$/, "") || root;
}

export interface Crumb {
  label: string;
  path: string;
}

export function crumbs(p: string): Crumb[] {
  const { root, parts } = splitPath(p);
  const sep = sepOf(p);
  const out: Crumb[] = [];
  if (root) out.push({ label: rootLabel(root), path: root });
  let acc = root;
  for (const part of parts) {
    acc = acc === "" ? part : acc.endsWith(sep) ? acc + part : acc + sep + part;
    out.push({ label: part, path: acc });
  }
  return out;
}

// ── Small helpers ───────────────────────────────────────────────────────────

/// Index of the next name (from `from`, wrapping) that starts with `query`.
/// Case- and composition-insensitive, so `가` finds a decomposed macOS name.
export function typeAhead(names: readonly string[], query: string, from: number): number {
  const q = looseKey(query);
  if (!q || names.length === 0) return -1;
  for (let k = 0; k < names.length; k++) {
    const i = (clamp(from, names.length) + k) % names.length;
    if (looseKey(names[i]).startsWith(q)) return i;
  }
  return -1;
}

/// End of the part of a name a rename should pre-select: everything before
/// the last extension. A directory or a dotfile has no extension.
export function stemEnd(name: string, isDir: boolean): number {
  if (isDir) return name.length;
  const dot = name.lastIndexOf(".");
  return dot > 0 ? dot : name.length;
}

/// Same thresholds and one-decimal format as `fsx::format_size` / ydir.
export function formatSize(size: number): string {
  const K = 1024;
  if (size < K) return `${size} B`;
  if (size < K * K) return `${(size / K).toFixed(1)} KB`;
  if (size < K * K * K) return `${(size / K / K).toFixed(1)} MB`;
  return `${(size / K / K / K).toFixed(1)} GB`;
}
