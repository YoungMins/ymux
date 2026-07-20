/// Formatting for files dropped onto a terminal pane.
///
/// Tauri's drag-drop event hands us real filesystem paths, which we type into
/// the PTY so an in-pane CLI can act on them — the same idea as the Ctrl+V
/// image paste, which types the saved screenshot's path.
///
/// Paths are double-quoted so a directory containing spaces stays a single
/// shell argument. Windows forbids `"` in filenames, so no escaping is needed
/// beyond the wrapping quotes.
export function formatDroppedPaths(paths: readonly string[]): string {
  return paths
    .filter((p) => p.length > 0)
    .map((p) => `"${p}"`)
    .join(" ");
}
