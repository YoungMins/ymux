// Where a clicked terminal path goes: ymux's own Markdown viewer, or the OS.
//
// Pure, so the routing is testable without a pane. Only an existing
// Markdown *file* is kept in-app — it opens in the editor pane's rendered
// preview, in the viewer tab of the clicked terminal's group, which reads
// the text and never runs anything. Everything else, directories included,
// still goes to `open_path` unchanged, so the backend's reveal-instead-of-run
// policy (`fspath::should_reveal`) keeps deciding for every other file.

import type { ResolvedPath } from "../types";
import { languageForPath } from "../editor/editorModel";

export type PathOpenTarget = "viewer" | "os";

/// Decide on `resolved.absolute` — the path that will actually be opened —
/// not on the text the matcher saw. `hasViewer` is false for a pane that has
/// no workspace to open a viewer tab in.
export function pathOpenTarget(resolved: ResolvedPath, hasViewer: boolean): PathOpenTarget {
  if (!hasViewer || resolved.is_dir) return "os";
  return languageForPath(resolved.absolute) === "markdown" ? "viewer" : "os";
}
