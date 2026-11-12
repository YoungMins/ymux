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
///
/// `wsl`: the pane runs `wsl.exe`, whose shell is a Linux one, so a Windows
/// path is typed as its `/mnt/<drive>/...` view, POSIX-quoted. A path with no
/// such view (UNC, relative) is left out like any other untypeable path.
import { quotePathForShell, windowsPathToWsl, type ShellFamily } from "./shellQuote";

export interface DroppedPaths {
  /// What to type: the typeable paths, quoted and space-separated.
  text: string;
  /// Paths that were left out because they cannot be typed into this shell.
  refused: string[];
}

export function formatDroppedPathsChecked(
  paths: readonly string[],
  family: ShellFamily,
  wsl = false,
): DroppedPaths {
  const typed: string[] = [];
  const refused: string[] = [];
  for (const p of paths) {
    if (!p) continue;
    const linux = wsl ? windowsPathToWsl(p) : p;
    const q = linux === null ? null : quotePathForShell(linux, wsl ? "posix" : family);
    if (q === null) refused.push(p);
    else typed.push(q);
  }
  return { text: typed.join(" "), refused };
}

export function formatDroppedPaths(
  paths: readonly string[],
  family: ShellFamily,
  wsl = false,
): string {
  return formatDroppedPathsChecked(paths, family, wsl).text;
}
