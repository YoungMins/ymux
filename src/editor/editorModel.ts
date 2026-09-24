// The editor pane's decisions, pure (spec §3.4). No DOM, no IPC, no
// CodeMirror values — `EditorPane.ts` imports this statically, and anything
// here that pulled CM6 in would put the whole library in the boot bundle.
//
// The rules that keep an edit from being lost live here, where they are
// tested: when the buffer is dirty, what an on-disk change means for it,
// whether opening another file in the same pane must ask first.

import { saveEol, type Eol } from "./eol";

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
///
/// The same file in a pane whose load failed is `retry`: read it again (and
/// re-offer any draft) — never treated as "replace", which would discard.
export function openFileDecision(
  current: string | null,
  dirty: boolean,
  next: string,
  loaded = true,
): "focus" | "retry" | "open" | "ask" {
  if (current !== null && current.normalize("NFC") === next.normalize("NFC")) {
    return loaded ? "focus" : "retry";
  }
  return dirty ? "ask" : "open";
}

export interface BufferState {
  /// A file is loaded (not loading, not an error).
  loaded: boolean;
  readOnly: boolean;
  /// The buffer differs from the last load/save.
  dirty: boolean;
  /// The file vanished from disk: the buffer is now the only copy.
  deletedOnDisk: boolean;
  /// A recovered draft is offered and not yet restored or discarded: the
  /// only copy of edits from a crashed session.
  pendingDraft: boolean;
  hasPath: boolean;
}

/// What closing this pane would lose, and whether "Save" can protect it.
///
/// A pending draft counts as unsaved even over a clean buffer — closing
/// would delete the only copy — but Save cannot protect it (it would write
/// the buffer, not the draft), so the prompt offers Discard/Cancel.
export function closeState(s: BufferState): { unsaved: boolean; savable: boolean } {
  if (s.pendingDraft) return { unsaved: true, savable: false };
  if (!s.loaded || s.readOnly) return { unsaved: false, savable: false };
  const unsaved = s.dirty || s.deletedOnDisk;
  return { unsaved, savable: unsaved && s.hasPath };
}

/// What to do with this pane's draft file after an edit, a save or a
/// reload. While a recovered draft is pending the file is left alone —
/// typing, undoing to clean, saving, or an agent's rewrite reloading the
/// buffer must not overwrite or delete the only copy of those edits before
/// the user has answered Restore / Discard.
///
/// `unanswered` is true both while a recovered draft is offered and before
/// the pane has even looked for one: a draft nobody has been offered is
/// never overwritten or deleted.
///
/// `deletedOnDisk` counts like `dirty`: once the file is gone the buffer is
/// the only copy, clean or not, so it is drafted — and typing then undoing
/// back to "clean" must not delete that draft.
export function draftFileAction(
  unanswered: boolean,
  dirty: boolean,
  deletedOnDisk = false,
): "keep" | "write" | "delete" {
  if (unanswered) return "keep";
  return dirty || deletedOnDisk ? "write" : "delete";
}

export interface WriteStamp {
  modified_ms: number;
  sha256: string;
}

/// The arguments of a save (`fs_write_text`). The guard is the stamp the
/// buffer was loaded or last saved from — so a file changed underneath
/// comes back `conflict` — and is dropped (`null`, an unconditional write)
/// only to recreate a file that was deleted and is confirmed still gone.
/// The BOM goes back as it was read; a mixed-EOL file is written as LF.
export function writeArgsFor(o: {
  path: string;
  text: string;
  eol: Eol;
  bom: boolean;
  stamp: WriteStamp;
  goneOnDisk: boolean;
}): { path: string; text: string; eol: Eol; bom: boolean; expect: WriteStamp | null } {
  return {
    path: o.path,
    text: o.text,
    eol: saveEol(o.eol),
    bom: o.bom,
    expect: o.goneOnDisk ? null : o.stamp,
  };
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
