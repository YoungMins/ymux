// The key buttons under the office peek's screen: enough to answer an agent's
// menu or dialog (move, pick a number, confirm, back out) without leaving the
// office. Pure: a key name to the bytes a terminal would send for it.

export type PeekKey = "up" | "down" | "enter" | "esc" | "tab" | "1" | "2" | "3";

export const PEEK_KEYS: readonly { key: PeekKey; label: string }[] = [
  { key: "up", label: "↑" },
  { key: "down", label: "↓" },
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
