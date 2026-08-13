/// A workspace's `name` is "custom" only when the user actually renamed it —
/// the auto-assigned `workspace-<id>` and the legacy "main" default do not
/// count, so those render as just the number.
function isCustomName(id: number, name: string | null | undefined): name is string {
  return !!name && name !== `workspace-${id}` && name !== "main";
}

/// Label for a workspace tab/row: `"1: build"` when custom-named, else `"1"`.
export function formatWorkspaceLabel(
  id: number,
  name: string | null | undefined,
): string {
  return isCustomName(id, name) ? `${id}: ${name}` : String(id);
}
