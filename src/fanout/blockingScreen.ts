// Pure detector for an agent CLI screen that needs a human answer: a folder
// trust / first-run dialog or any numbered select menu. Fan-out must never
// paste or press Enter into one (owner decision: ymux never pre-accepts an
// agent's trust dialog), so the ready gate stands down while this matches.

export type BlockKind = "trust" | "select";

/// Rows of the visible screen the caller should look at.
export const SCAN_ROWS = 30;

/// Phrases only a trust / first-run dialog prints (lowercase). One is enough.
/// Words like "trust" or "approve" alone are deliberately absent: they occur
/// in ordinary prose and code output. A real approval menu is caught by the
/// select shape below.
const STRONG = [
  "do you trust the files in this folder",
  "do you trust the contents of this directory",
  "do you trust this folder",
  "do you trust this directory",
  "trust this folder",
  "trust this directory",
  "allow codex to work in this folder",
  "quick safety check",
  "is this a project you created or one you trust",
  "yes, proceed",
];

/// A selection cursor on a numbered option: `❯ 1. Yes`, `› 2) No`, also
/// inside a box border (`│ ❯ 1. Yes │`).
const SELECT = /^[\s│┃|]*[❯›]\s*\d+[.)]\s+\S/;

/// How much of the pasted text's first line identifies its echo.
const ECHO_CHARS = 40;
const ECHO_MIN = 4;

/// `lines` without the rows that echo the start of `pasted` (the agent shows
/// our own prompt back, e.g. Codex's `› 1. do x`, which looks like a menu).
export function dropEchoRows(lines: readonly string[], pasted: string): string[] {
  const first = pasted.split(/\r?\n/).find((l) => l.trim() !== "") ?? "";
  const needle = first.trim().slice(0, ECHO_CHARS).trim();
  if (needle.length < ECHO_MIN) return [...lines];
  return lines.filter((row) => !row.includes(needle));
}

/// `echoOf`: text this program pasted into the pane; rows echoing it are not
/// evidence of a dialog.
export function detectBlockingScreen(lines: readonly string[], echoOf?: string): BlockKind | null {
  const rows = (echoOf ? dropEchoRows(lines, echoOf) : [...lines]).slice(-SCAN_ROWS);
  for (const row of rows) {
    const low = row.toLowerCase();
    if (STRONG.some((p) => low.includes(p))) return "trust";
  }
  for (const row of rows) if (SELECT.test(row)) return "select";
  return null;
}
