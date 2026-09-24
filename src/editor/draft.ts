// Local drafts of unsaved editor content (spec §3.5, "belt and braces").
//
// A dirty buffer is written, 2 s after the last edit, to a per-pane file
// beside the scrollback files (`save_editor_draft`). It is a safety net for
// the close paths the guard cannot see — a crash, a killed process, macOS
// Cmd+Q — never an autosave: the real file is only ever written by an
// explicit save. Lifecycle is scrollback's: deleted on a permanent close
// and whenever the buffer becomes clean, offered back on the next mount.
//
// The draft carries the stamp the buffer was based on. Restoring puts that
// stamp back as the save guard, so if the file changed on disk since the
// draft was taken the next save hits `Conflict` and asks — a recovered draft
// can never silently overwrite what an agent wrote in the meantime.

import type { Eol } from "./eol";

export interface Stamp {
  modified_ms: number;
  sha256: string;
}

export interface Draft {
  v: 1;
  path: string;
  text: string;
  eol: Eol;
  bom: boolean;
  base: Stamp;
  savedAt: number;
}

const EOLS: readonly string[] = ["lf", "crlf", "cr", "mixed", "none"];

export function encodeDraft(d: Draft): string {
  return JSON.stringify(d);
}

/// A draft read back from disk, or `null` for empty / foreign / corrupt
/// content. Total: never throws.
export function parseDraft(blob: string): Draft | null {
  if (!blob) return null;
  let v: unknown;
  try {
    v = JSON.parse(blob);
  } catch {
    return null;
  }
  if (!v || typeof v !== "object") return null;
  const d = v as Record<string, unknown>;
  const base = d.base as Record<string, unknown> | undefined;
  if (
    d.v !== 1 ||
    typeof d.path !== "string" ||
    typeof d.text !== "string" ||
    typeof d.eol !== "string" ||
    !EOLS.includes(d.eol) ||
    typeof d.bom !== "boolean" ||
    !base ||
    typeof base.sha256 !== "string" ||
    typeof base.modified_ms !== "number"
  ) {
    return null;
  }
  return {
    v: 1,
    path: d.path,
    text: d.text,
    eol: d.eol as Eol,
    bom: d.bom,
    base: { modified_ms: base.modified_ms, sha256: base.sha256 },
    savedAt: typeof d.savedAt === "number" ? d.savedAt : 0,
  };
}

/// What the pane's load found on disk, for the draft decision. `null` = the
/// read failed (the file is gone, renamed, locked, no longer text).
export interface DiskView {
  text: string;
  /// The buffer can take the draft: loaded and not read-only (a file past
  /// the 8 MB cap loads read-only and cannot).
  editable: boolean;
}

/// What to do with the draft found for a pane (spec §3.5, and the review's
/// "the draft is the only copy" finding):
///
/// - `none`    — there is no draft;
/// - `drop`    — it says exactly what the disk says: nothing to lose;
/// - `restore` — offer Restore / Discard into the loaded buffer;
/// - `rescue`  — offer Save as… / Discard: the buffer cannot take it (the
///               read failed, or the file is read-only now), so the draft is
///               written to a file the user picks instead.
///
/// It is never silently deleted just because the load went wrong.
export type DraftDisposition = "none" | "drop" | "restore" | "rescue";

export function draftDisposition(
  draft: Draft | null,
  path: string,
  disk: DiskView | null,
): DraftDisposition {
  if (!draft) return "none";
  if (draft.path.normalize("NFC") !== path.normalize("NFC")) return "drop";
  if (!disk || !disk.editable) return "rescue";
  return draft.text === disk.text ? "drop" : "restore";
}
