/// Pure merge behind the shell list refresh: the backend rescans shells in the
/// background at startup and only ever ADDS newly installed ones, so the
/// frontend keeps its own order (default shell first) and appends what is new.
import type { ShellProfile } from "../types";

/// `known` followed by every profile of `fresh` whose name `known` lacks.
/// Returns `known` itself (same array) when nothing is new.
export function mergeNewShells(
  known: readonly ShellProfile[],
  fresh: readonly ShellProfile[],
): ShellProfile[] {
  const names = new Set(known.map((s) => s.name));
  const added = fresh.filter((s) => !names.has(s.name));
  return added.length === 0 ? (known as ShellProfile[]) : [...known, ...added];
}
