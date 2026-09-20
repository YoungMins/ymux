/// The pane the user works in: the focused pane when it belongs to the
/// active workspace (`paneIds`), else that workspace's first pane.
/// `focusedId` alone is not enough, because it keeps pointing into the
/// previous workspace after a switch until something is clicked.
export function pickActivePaneId(
  paneIds: readonly string[],
  focusedId: string | null,
): string | null {
  if (focusedId && paneIds.includes(focusedId)) return focusedId;
  return paneIds[0] ?? null;
}
