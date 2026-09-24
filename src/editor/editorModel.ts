// The editor pane's decisions, pure (spec §3.4). No DOM, no IPC, no
// CodeMirror values — `EditorPane.ts` imports this statically, and anything
// here that pulled CM6 in would put the whole library in the boot bundle.
//
// The rules that keep an edit from being lost live here, where they are
// tested: when the buffer is dirty, what an on-disk change means for it,
// whether opening another file in the same pane must ask first.

/// A grammar the editor ships in v1 (spec §3.3).
export type LangId =
  | "rust"
  | "javascript"
  | "jsx"
  | "typescript"
  | "tsx"
  | "json"
  | "html"
  | "css"
  | "markdown"
  | "python"
  | "yaml";

const EXT_LANG: Record<string, LangId> = {
  rs: "rust",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  json: "json",
  jsonc: "json",
  html: "html",
  htm: "html",
  // No Lezer grammar for these; html covers the markup (spec §3.1).
  svelte: "html",
  vue: "html",
  css: "css",
  md: "markdown",
  markdown: "markdown",
  py: "python",
  pyw: "python",
  pyi: "python",
  yml: "yaml",
  yaml: "yaml",
};

/// The last path component, for either separator. Display only.
export function fileName(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut >= 0 ? path.slice(cut + 1) : path;
}

/// Which grammar a file gets, from its extension (case-insensitive). A
/// dotfile with no further dot (`.bashrc`) and a name with no extension
/// are plain text.
export function languageForPath(path: string): LangId | null {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return null;
  return EXT_LANG[name.slice(dot + 1).toLowerCase()] ?? null;
}

/// What a document needs to support for the dirty check. CodeMirror's
/// `Text` satisfies it; tests can too.
export interface DocLike {
  readonly length: number;
  eq(other: DocLike): boolean;
}

/// Is the buffer different from what was last loaded or saved?
///
/// Content, not history: typing a character and deleting it again is
/// clean, and so is undoing back to the saved text — the bug ycode has
/// (its `undo` sets `dirty = true` unconditionally). The length check
/// first keeps a keystroke in an 8 MB file from comparing 8 MB.
export function isDocDirty(saved: DocLike, current: DocLike): boolean {
  if (saved === current) return false;
  if (saved.length !== current.length) return true;
  return !saved.eq(current);
}

/// The focus poll's first step: is the file's mtime what we last stamped?
/// `null` stat = the file is gone. A changed mtime is only a *maybe* — the
/// content hash decides (see `conflictDecision`).
export function pollStep(
  stat: { modified_ms: number } | null,
  stampMtime: number,
): "unchanged" | "read" | "missing" {
  if (!stat) return "missing";
  return stat.modified_ms === stampMtime ? "unchanged" : "read";
}

export type ConflictDecision = "none" | "reload" | "prompt" | "deleted";

/// The file on disk vs. the stamp the buffer was loaded/saved from
/// (spec §3.6). Compared on the hash, exactly as `fs_write_text` does, so a
/// `touch` or an identical rewrite is not a change.
///
/// - unchanged → nothing to do, whatever the buffer holds;
/// - changed + clean buffer → reload silently (nothing of the user's to lose);
/// - changed + dirty buffer → ask (never pick a winner silently);
/// - gone → say so; the buffer is now the only copy.
export function conflictDecision(
  diskSha: string | null,
  expectedSha: string,
  dirty: boolean,
): ConflictDecision {
  if (diskSha === null) return "deleted";
  if (diskSha === expectedSha) return "none";
  return dirty ? "prompt" : "reload";
}

/// Opening `next` in a pane that shows `current` (the viewer tab's reuse,
/// spec §3.7): the same file just takes focus; another file replaces a clean
/// buffer, and must ask first over a dirty one.
///
/// "Same" is NFC-equal only — rule 15 keeps case and separator folding on
/// the Rust side. A respelling that slips through is treated as another
/// file, which costs at most a needless save prompt, never an edit.
export function openFileDecision(
  current: string | null,
  dirty: boolean,
  next: string,
): "focus" | "open" | "ask" {
  if (current !== null && current.normalize("NFC") === next.normalize("NFC")) return "focus";
  return dirty ? "ask" : "open";
}

/// Where the cursor goes after a silent reload: the same line (clamped to
/// the new length) and the same column (clamped to that line). 1-based line.
export function cursorAfterReload(
  line: number,
  col: number,
  lineCount: number,
  lineLength: (line: number) => number,
): { line: number; col: number } {
  const l = Math.min(Math.max(1, line), Math.max(1, lineCount));
  return { line: l, col: Math.min(Math.max(0, col), lineLength(l)) };
}

/// The `n`th "save as copy" candidate beside `path`: `a (copy).rs`, then
/// `a (copy 2).rs`, … Keeps the original's separator and extension; a
/// dotfile keeps its whole name as the stem.
export function copyCandidate(path: string, n: number): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dir = path.slice(0, cut + 1);
  const name = path.slice(cut + 1);
  const dot = name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  const tag = n <= 1 ? "copy" : `copy ${n}`;
  return `${dir}${stem} (${tag})${ext}`;
}
