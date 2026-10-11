/// A workspace's `name` is "custom" only when the user actually renamed it —
/// the auto-assigned `workspace-<id>` and the legacy "main" default do not
/// count, so those render as just the number.
function isCustomName(id: number, name: string | null | undefined): name is string {
  return !!name && name !== `workspace-${id}` && name !== "main";
}

/// The number users see and type is the workspace's 1-based POSITION in the
/// panel (`config.workspaces` order), never its persistent `id` — ids have gaps
/// after a delete and keep keying scrollback/notes/drafts. Returns 0 if absent.
export function workspacePosition(
  workspaces: readonly { id: number }[],
  id: number,
): number {
  return workspaces.findIndex((w) => w.id === id) + 1;
}

/// Inverse of `workspacePosition`: the id at 1-based `position`, or null when
/// out of range (Ctrl+Alt+N with fewer than N workspaces does nothing).
export function workspaceIdAtPosition(
  workspaces: readonly { id: number }[],
  position: number,
): number | null {
  return workspaces[position - 1]?.id ?? null;
}

/// Label for a workspace tab/row: `"1: build"` when custom-named, else `"1"`.
/// `position` is the number shown; `id` only identifies the auto-assigned
/// `workspace-<id>` default name (an "unnamed" sentinel, never displayed).
export function formatWorkspaceLabel(
  id: number,
  name: string | null | undefined,
  position: number,
): string {
  return isCustomName(id, name) ? `${position}: ${name}` : String(position);
}

/// What the in-place rename editor starts with. A workspace that was never
/// renamed edits from an *empty* box rather than from `workspace-3`: the row
/// only ever showed `3`, so pre-filling the machine-generated name would ask
/// the user to delete a string they never typed.
export function editableWorkspaceName(
  id: number,
  name: string | null | undefined,
): string {
  return isCustomName(id, name) ? name : "";
}

/// Validate one in-place rename. Returns the name to store, or `null` when the
/// edit is rejected and the old name must survive.
///
/// Rejected: empty or whitespace-only (the row would lose its label for no
/// gain), and a no-op edit (so a stray double-click + Enter never dirties the
/// config). `current` is an `editableWorkspaceName` value, so comparing
/// against it treats "still unnamed" as a no-op too.
export function nextWorkspaceName(
  raw: string,
  current: string,
): string | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;
  return trimmed === current ? null : trimmed;
}
