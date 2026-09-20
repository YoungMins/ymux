// What a re-render has to do about an active pane zoom.
//
// `toggleZoomFocused` implements zoom in CSS: the workspace container gets
// `workspace--zoomed` (which hides every direct child `.split` / `.pane-group`)
// and the zoomed element is re-parented to the container and made an overlay.
// The layout tree is untouched — but `renderWorkspace` rebuilds the container's
// children, so every later render throws that re-parenting away while the
// container still carries `workspace--zoomed`, leaving the workspace blank
// until the user pressed `Ctrl+Shift+Z` a second time. Any tab operation
// (select / step / new / the dock's viewer tab) renders, and so did closing or
// swapping a pane.
//
// So the manager re-decides after every render, with the DOM work kept out of
// here: this is the decision, and it is pure.

import type { Uuid } from "../types";

export type ZoomAction =
  /// Nothing is zoomed in this workspace; leave the render alone.
  | { kind: "none" }
  /// The zoomed pane is gone (closed, or its workspace was rebuilt without
  /// it). The container's `workspace--zoomed` class must come off, or its
  /// siblings stay `display: none` and the workspace renders empty.
  | { kind: "clear" }
  /// Re-apply the zoom. `groupId` is looked up *now*, not remembered: a pane
  /// can gain tabs (zoom the group, so the shared chrome comes with it) or
  /// lose them ("close other tabs" unwraps the group) while zoomed.
  | { kind: "apply"; paneId: Uuid; groupId: Uuid | null };

export function zoomAction(
  zoomedPaneId: Uuid | null,
  paneExists: boolean,
  groupId: Uuid | null,
): ZoomAction {
  if (!zoomedPaneId) return { kind: "none" };
  if (!paneExists) return { kind: "clear" };
  return { kind: "apply", paneId: zoomedPaneId, groupId };
}
