// "One viewer tab per pane, reused" (spec §4). The dock's second Enter must
// replace the file in the tab that is already open, not pile tabs up — but
// the user can close that tab at any time, so the registry is advisory and
// every decision is re-validated against the group's current members.

import type { Uuid } from "../types";

export type ViewerAction = { kind: "reuse"; paneId: Uuid } | { kind: "create" };

/// `registered` is the viewer tab remembered for this group (if any),
/// `members` the group's tab ids right now.
export function viewerTabAction(
  registered: Uuid | undefined,
  members: readonly Uuid[],
): ViewerAction {
  if (registered && members.includes(registered)) {
    return { kind: "reuse", paneId: registered };
  }
  return { kind: "create" };
}
