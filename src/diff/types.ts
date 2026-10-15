// TypeScript mirror of the diff-review structs in src-tauri/src/git/mod.rs.
// Field names are snake_case: the Rust structs carry no serde rename, like
// the other git types in ipc/bridge.ts.

/// `git::DiffFile`: one changed file.
export interface DiffFile {
  path: string;
  /// A rename's old path; empty otherwise.
  orig: string;
  added: number;
  deleted: number;
  binary: boolean;
  untracked: boolean;
  /// An untracked file whose lines were not counted (the summary's read
  /// budget ran out); `added` is 0 and the list shows "?".
  counts_unknown: boolean;
}

/// `git::DiffSummary`.
export interface DiffSummary {
  root: string;
  /// Full hash compared against; pass it back to `gitApi.diffFile`.
  base_rev: string;
  files: DiffFile[];
  /// More than 3000 files differ; the list is cut.
  truncated: boolean;
}

/// `git::DiffLineKind`. `noeol` is `\ No newline at end of file` and
/// describes the line above it.
export type DiffLineKind = "ctx" | "add" | "del" | "noeol";

/// `git::DiffLine`.
export interface DiffLine {
  kind: DiffLineKind;
  /// 1-based; null for an added line and for `noeol`.
  old_no: number | null;
  /// 1-based; null for a deleted line and for `noeol`.
  new_no: number | null;
  /// No marker, no trailing `\r`, clipped to 2000 chars (ending in `…`).
  text: string;
  /// The line ended in CRLF.
  cr: boolean;
}

/// `git::Hunk`.
export interface Hunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  /// Text after the closing `@@`; may be empty.
  header: string;
  lines: DiffLine[];
}

/// `git::FileDiff`.
export interface FileDiff {
  hunks: Hunk[];
  binary: boolean;
  /// The patch was cut at 512 KiB (or the untracked file at 256 KiB).
  truncated: boolean;
}
