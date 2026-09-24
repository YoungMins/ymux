// The TS half of the line-ending model (spec §1.2, §3.4). The authority is
// `src-tauri/src/textfile.rs`: `fs_read_text` detects the file's ending and
// hands the editor `\n`-only text, `fs_write_text` puts the ending back. The
// editor itself only ever holds `\n` — CodeMirror's `doc.toString()` joins
// lines with `\n`, so what the buffer gives back is exactly what the backend
// expects.
//
// This mirror exists for the decisions the pane makes on the way: what a
// save will write (`saveEol`), whether to warn first (`needsEolWarning`),
// how to label the status bar. `detectEol`/`normalizeToLf`/`restoreEol` are
// the Rust functions ported one-for-one so the tests can round-trip a file
// through the editor's own document model and prove the bytes come back.

/// Matches `textfile::Eol` (serde lowercase).
export type Eol = "lf" | "crlf" | "cr" | "mixed" | "none";

/// Which line ending dominates `s`. More than one kind is `mixed`.
export function detectEol(s: string): Eol {
  const crlf = s.split("\r\n").length - 1;
  const rest = s.split("\r\n").join("");
  const lf = rest.split("\n").length - 1;
  const cr = rest.split("\r").length - 1;
  const kinds = [crlf > 0, lf > 0, cr > 0].filter(Boolean).length;
  if (kinds === 0) return "none";
  if (kinds > 1) return "mixed";
  if (crlf > 0) return "crlf";
  return lf > 0 ? "lf" : "cr";
}

/// Fold every ending to `\n` (CRLF first, then a lone CR).
export function normalizeToLf(s: string): string {
  return s.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
}

/// The ending a save writes for a file read as `eol`. `mixed` becomes LF —
/// the one case where saving deliberately changes endings, so the pane
/// asks first (`needsEolWarning`).
export function saveEol(eol: Eol): Eol {
  return eol === "mixed" ? "lf" : eol;
}

/// Re-apply `eol` to `\n` text.
export function restoreEol(s: string, eol: Eol): string {
  const e = saveEol(eol);
  if (e === "crlf") return s.replace(/\n/g, "\r\n");
  if (e === "cr") return s.replace(/\n/g, "\r");
  return s;
}

/// A save of this file rewrites line endings the user did not touch.
export function needsEolWarning(eol: Eol): boolean {
  return eol === "mixed";
}

/// Status-bar label. Not translated: these are the names every editor uses.
/// A file with no line ending yet saves as LF if the user adds one — which
/// is what `textfile::encode` does — so it reads "LF".
export function eolLabel(eol: Eol): string {
  switch (eol) {
    case "crlf":
      return "CRLF";
    case "cr":
      return "CR";
    case "mixed":
      return "Mixed";
    default:
      return "LF";
  }
}
