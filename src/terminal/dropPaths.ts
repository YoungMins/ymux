/// Formatting for files dropped onto a terminal pane.
///
/// Tauri's drag-drop event hands us real filesystem paths, which we type into
/// the PTY so an in-pane CLI can act on them — the same idea as the Ctrl+V
/// image paste, which types the saved screenshot's path.
///
/// Each path is quoted for the pane's shell (`shellQuote.ts`) so spaces stay
/// one argument and `$`, backticks or `\` in a name are never expanded. A
/// path that cannot be typed safely (a control character in its name, or a
/// character the pane's unknown shell might interpret) is
/// left out rather than typed.
import { quotePathForShell, type ShellFamily } from "./shellQuote";

export function formatDroppedPaths(
  paths: readonly string[],
  family: ShellFamily,
): string {
  return paths
    .map((p) => quotePathForShell(p, family))
    .filter((q): q is string => q !== null)
    .join(" ");
}
