// The key buttons under the office peek's screen: enough to answer an agent's
// menu or dialog (move, pick a number, confirm, back out) without leaving the
// office. Pure: a key name to the bytes a terminal would send for it.

export type PeekKey = "up" | "down" | "left" | "right" | "enter" | "esc" | "tab" | "1" | "2" | "3";

export const PEEK_KEYS: readonly { key: PeekKey; label: string }[] = [
  { key: "up", label: "↑" },
  { key: "down", label: "↓" },
  { key: "left", label: "←" },
  { key: "right", label: "→" },
  { key: "enter", label: "Enter" },
  { key: "esc", label: "Esc" },
  { key: "tab", label: "Tab" },
  { key: "1", label: "1" },
  { key: "2", label: "2" },
  { key: "3", label: "3" },
];

/// The bytes for `key`. Arrows follow the app's cursor-key mode (DECCKM):
/// full-screen apps that turn it on expect `ESC O A`, everything else `ESC [ A`.
export function keyBytes(key: PeekKey, applicationCursor: boolean): string {
  switch (key) {
    case "up":
      return applicationCursor ? "\x1bOA" : "\x1b[A";
    case "down":
      return applicationCursor ? "\x1bOB" : "\x1b[B";
    case "right":
      return applicationCursor ? "\x1bOC" : "\x1b[C";
    case "left":
      return applicationCursor ? "\x1bOD" : "\x1b[D";
    case "enter":
      return "\r";
    case "esc":
      return "\x1b";
    case "tab":
      return "\t";
    default:
      return key;
  }
}

/// One choice of a menu an agent shows (`❯ 1. Yes`): its number, its text,
/// and whether the menu's pointer is on it.
export interface MenuOption {
  n: string;
  label: string;
  selected: boolean;
  /// Picking it opens a text field in the agent ("No, and tell Claude what
  /// to do differently", "Type something", "Other"): the peek asks for the
  /// text first and sends both.
  input: boolean;
}

/// Labels of choices that take typed text, in the agents' own wording and a
/// few translations of "other / type it yourself".
const INPUT_OPTION =
  /tell \S+ (what to do|differently)|what to do differently|type (something|your own|an answer)|something else|^other\b|custom|직접 입력|기타|다른 (답|내용)|自分で入力|その他|其他|自定义|otro|autre|outro|другое|diğer|andere/i;

export function isInputOption(label: string): boolean {
  return INPUT_OPTION.test(label.trim());
}

/// A screen line without the box it may be drawn in (`│ ❯ 1. Yes │`).
function unframe(line: string): string {
  return line.replace(/^[\s│┃║|]+/, "").replace(/[\s│┃║|]+$/, "");
}

const OPTION = /^\s*([❯›>→▶])?\s*(\d{1,2})[.)]\s+(\S.*?)\s*$/;
/// Rows from the bottom of the screen a menu is looked for in.
const MENU_ROWS = 30;

/// The menu at the bottom of `lines`, if one is up: a run of numbered rows
/// 1, 2, 3, … one of which carries a pointer (❯ › > → ▶). The pointer is what
/// tells a live menu from a numbered list in the agent's answer. Rows between
/// the options (a description under one) are allowed.
export function menuOptions(lines: readonly string[]): MenuOption[] {
  const tail = lines.slice(-MENU_ROWS);
  let best: MenuOption[] = [];
  let run: MenuOption[] = [];
  for (const line of tail) {
    const m = OPTION.exec(unframe(line));
    if (!m) continue;
    const opt = { n: m[2], label: m[3], selected: !!m[1], input: isInputOption(m[3]) };
    if (Number(opt.n) === run.length + 1) run.push(opt);
    else if (opt.n === "1") run = [opt];
    else continue;
    if (run.length >= 2 && run.some((o) => o.selected)) best = [...run];
  }
  return best;
}

/// What a key pressed with the peek's screen focused sends to the terminal,
/// or null for keys it leaves alone (modifiers, function keys, a key in the
/// middle of an IME composition). Typed characters go as themselves, Ctrl+A…Z
/// as control codes.
export function directKey(
  ev: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; isComposing?: boolean },
  applicationCursor: boolean,
): string | null {
  if (ev.isComposing || ev.key === "Process" || ev.key === "Unidentified" || ev.metaKey) return null;
  const arrows: Record<string, PeekKey> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right" };
  if (arrows[ev.key]) return keyBytes(arrows[ev.key], applicationCursor);
  switch (ev.key) {
    case "Enter":
      return "\r";
    case "Backspace":
      return "\x7f";
    case "Tab":
      return ev.shiftKey ? "\x1b[Z" : "\t";
    case "Escape":
      return "\x1b";
    case "Delete":
      return "\x1b[3~";
    case "Home":
      return "\x1b[H";
    case "End":
      return "\x1b[F";
  }
  if (ev.key.length !== 1) return null;
  if (ev.ctrlKey && !ev.altKey) {
    const c = ev.key.toLowerCase().charCodeAt(0);
    return c >= 97 && c <= 122 ? String.fromCharCode(c - 96) : null;
  }
  return ev.altKey ? `\x1b${ev.key}` : ev.key;
}

/// The question a menu at the bottom of `lines` answers: up to `max` lines
/// just above its first choice, frames and blanks dropped ("Do you want to
/// run `rm -rf build`?"). Empty when there is no menu.
export function menuQuestion(lines: readonly string[], max = 3): string[] {
  const opts = menuOptions(lines);
  if (opts.length === 0) return [];
  const tail = lines.slice(-30);
  const first = tail.findIndex((l) => {
    const m = /^[❯›>→▶]?\s*(\d{1,2})[.)]\s+/.exec(unframe(l));
    return m !== null && m[1] === opts[0].n && l.includes(opts[0].label);
  });
  if (first <= 0) return [];
  const out: string[] = [];
  for (let i = first - 1; i >= 0 && out.length < max; i--) {
    const text = tail[i].replace(/[│┃║╭╮╰╯─━═┌┐└┘|]/g, "").trim();
    if (text === "") {
      if (out.length > 0) break;
      continue;
    }
    out.unshift(text);
  }
  return out;
}
