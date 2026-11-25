// Remote server: is the agent blocked on a dialog, as far as a phone should be
// told. Two pure pieces: the screen test (stricter than fan-out's
// `detectBlockingScreen`, which must err on the side of not pasting) and the
// debounce that keeps one odd frame from flipping a pane's status.

import { dropEchoRows, STRONG } from "../fanout/blockingScreen";
import { findMenu } from "../office/peekKeys";

/// Rows from the bottom a dialog must be in. A menu answered a while ago is
/// still in the scrollback; only the last rows are the live screen.
export const DIALOG_ROWS = 20;
/// Rows from the bottom a known trust / first-run phrase must be in.
export const PHRASE_ROWS = 6;

/// A row that may stand under a pending menu: nothing, a box border, the
/// dialog's own hint ("Esc to cancel", "Enter to select · ↑/↓ to navigate",
/// "Press enter to confirm or esc to cancel") or the agent's footer ("gpt-5 ·
/// 100% context left", "? for shortcuts"). A working indicator ("esc to
/// interrupt"), an agent's output row or a fresh input line is none of these.
const BORDER = /^[\s─━═╭╮╰╯┌┐└┘├┤┬┴┼\-_=+]*$/;
const HINT = /^(press\s+)?(esc|escape|enter|tab)\b[^.]*\bto\b|\bpress (enter|esc)\b|[↑↓]|arrow keys|to navigate/i;
const FOOTER = /context left|for shortcuts|shift\+tab/i;

function trailerKind(row: string): "blank" | "hint" | "footer" | "continuation" | "other" {
  const body = row.replace(/^\s*[│┃║|]/, "").replace(/[│┃║|]\s*$/, "");
  const t = body.trim();
  if (t === "" || BORDER.test(t)) return "blank";
  if (/interrupt/i.test(t)) return "other";
  if (HINT.test(t)) return "hint";
  if (FOOTER.test(t)) return "footer";
  // A description under the last option sits deeper than the options do.
  if (/^ {4,}\S/.test(body)) return "continuation";
  return "other";
}

/// The menu is a PENDING question, not history: after its last option row
/// there is only blank rows, borders, its own hints, footer chrome and option
/// descriptions. Anything else (the agent's output, a working indicator, a
/// fresh input line) means the question was answered or never was one: the
/// user's own numbered prompt echoed back (`› 1. a` / `  2. b`) is followed by
/// exactly that. A plain `>` pointer is what Claude Code draws on the user's
/// own message (real menus use `❯`), so it needs a dialog hint to count.
function isPending(after: readonly string[], pointer: string): boolean {
  const kinds = after.map(trailerKind);
  if (kinds.includes("other")) return false;
  return pointer !== ">" || kinds.includes("hint");
}

/// The screen (`rows`: one string per terminal row, trailing blanks dropped)
/// shows a real dialog: a numbered menu with a pointer on one option and at
/// least one more numbered option (the very shape the phone's answer buttons
/// are built from, `menuOptions`) that is still the last thing on the screen
/// (`isPending`), or one of the known blocking dialogs in the last few rows.
/// `echoOf`: the last prompt sent to the pane; rows echoing its first line
/// (`› 1. do x`) are not evidence of anything.
export function screenShowsDialog(rows: readonly string[], echoOf?: string): boolean {
  const lines = (echoOf ? dropEchoRows(rows, echoOf) : [...rows]).slice(-DIALOG_ROWS);
  const menu = findMenu(lines);
  if (menu.options.length >= 2 && isPending(lines.slice(menu.end + 1), menu.pointer)) return true;
  return lines
    .slice(-PHRASE_ROWS)
    .some((row) => STRONG.some((p) => row.toLowerCase().includes(p)));
}

/// A raw yes/no per pane is turned into a stable one: a change is believed
/// only when seen on two evaluations at least `gapMs` apart with nothing to
/// the contrary between them. Flapping input therefore never moves the state.
export class DialogDebounce {
  private panes = new Map<string, { stable: boolean; since: number | null }>();

  constructor(private readonly gapMs = 1500) {}

  /// One evaluation: the screen shows (`seen`) a dialog or not, at `now`.
  observe(id: string, seen: boolean, now: number): void {
    const e = this.panes.get(id) ?? { stable: false, since: null };
    this.panes.set(id, e);
    if (seen === e.stable) {
      e.since = null;
    } else if (e.since === null) {
      e.since = now;
    } else if (now - e.since >= this.gapMs) {
      e.stable = seen;
      e.since = null;
    }
  }

  /// The believed state; false for a pane never observed. No side effects.
  stable(id: string): boolean {
    return this.panes.get(id)?.stable ?? false;
  }

  /// Forget every pane not in `ids`.
  retainOnly(ids: Iterable<string>): void {
    const keep = new Set(ids);
    for (const id of [...this.panes.keys()]) if (!keep.has(id)) this.panes.delete(id);
  }

  /// Not evaluated now (the agent's status makes the screen irrelevant) or the
  /// pane is gone: start over.
  forget(id: string): void {
    this.panes.delete(id);
  }
}
