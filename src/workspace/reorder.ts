/// Pure list-reordering helpers behind the workspace panel's drag-to-reorder.
///
/// The panel renders `config.workspaces` in array order, so "reorder" is just
/// a splice of that Vec — it round-trips through TOML's `[[workspaces]]` array
/// for free, with no new model field and no CONFIG_VERSION bump.

/// Move `list[from]` so that it lands immediately before the item that is at
/// index `insertBefore` in the ORIGINAL list (`insertBefore === list.length`
/// means "append at the end"). Returns a new array, or `null` when the move
/// is out of range or a no-op — callers use `null` to skip a persist/rebuild.
export function moveItem<T>(
  list: readonly T[],
  from: number,
  insertBefore: number,
): T[] | null {
  if (!Number.isInteger(from) || from < 0 || from >= list.length) return null;
  const target = Math.max(0, Math.min(insertBefore, list.length));
  // Dropping just above or just below yourself changes nothing.
  if (target === from || target === from + 1) return null;
  const out = [...list];
  const [item] = out.splice(from, 1);
  // Removing the item shifts everything after it left by one.
  out.splice(target > from ? target - 1 : target, 0, item);
  return out;
}

/// Index the dragged row should be inserted *before*, given each row's
/// vertical midpoint (ascending, same coordinate space as `y`). Returns
/// `midpoints.length` when the pointer is past the last row.
export function insertIndexFromMidpoints(
  midpoints: readonly number[],
  y: number,
): number {
  let i = 0;
  while (i < midpoints.length && y > midpoints[i]) i += 1;
  return i;
}
