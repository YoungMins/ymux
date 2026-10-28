// Split dividers that line up: dragging one near a neighbouring divider of
// the same direction snaps it onto that divider's line, and dividers on one
// line that meet end to end then move together. Pure geometry over the
// dividers' boxes on screen; SplitContainer measures and applies.

/// A divider on screen. `at` is its centre across the drag (x for a
/// left/right divider, y for a top/bottom one); `from`/`to` its extent
/// along its length.
export interface GutterLine {
  at: number;
  from: number;
  to: number;
}

/// How close (CSS px) a divider must come to another to snap onto it.
export const SNAP_PX = 8;
/// Two lines closer than this are one line.
export const ALIGN_PX = 1.5;
/// The gap between two dividers meeting end to end: the crossing divider
/// between them, plus some slack.
export const TOUCH_PX = 8;

/// The ends of `a` and `b` meet (across the crossing divider), or overlap.
export function touches(a: GutterLine, b: GutterLine): boolean {
  return a.from <= b.to + TOUCH_PX && b.from <= a.to + TOUCH_PX;
}

/// The dividers that move with `lines[self]`: those on its line, joined to it
/// end to end, directly or through one another. Includes `self`.
export function linkedGroup(lines: readonly GutterLine[], self: number): number[] {
  const group = [self];
  const seen = new Set(group);
  for (let i = 0; i < group.length; i++) {
    const g = lines[group[i]];
    lines.forEach((l, j) => {
      if (seen.has(j) || Math.abs(l.at - g.at) > ALIGN_PX || !touches(l, g)) return;
      seen.add(j);
      group.push(j);
    });
  }
  return group;
}

/// Where a drag to `pos` lands: on a neighbour's line when one is within
/// `SNAP_PX` (the nearest), else at `pos`. Neighbours are the dividers not
/// in `group` that meet one of the group's dividers end to end.
export function snapTo(pos: number, lines: readonly GutterLine[], group: readonly number[]): { at: number; snapped: number | null } {
  const mine = new Set(group);
  let best: number | null = null;
  let dist = SNAP_PX + 1e-9;
  lines.forEach((l, j) => {
    if (mine.has(j) || !group.some((g) => touches(lines[g], l))) return;
    const d = Math.abs(l.at - pos);
    if (d <= dist) {
      dist = d;
      best = j;
    }
  });
  return best === null ? { at: pos, snapped: null } : { at: lines[best].at, snapped: best };
}
