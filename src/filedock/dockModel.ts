// Pure pieces of the right-side panel (src/sidepanel/SidePanel.ts): the
// persisted open/width/active-view state, the view-toggle transition and the
// width clamp used while dragging.

/// Widths are in px. They were sized for yDir's terminal columns, which is
/// the history below; the files pane that replaced it drops its date column
/// under 380 px and its size column under 270 px, so the same two numbers
/// still mean "full listing" and "names plus sizes". At the default
/// 13 px terminal font a monospace cell is ~7.6 px wide, and the dock
/// spends 2 cells on yDir's panel border plus ~6 px on its resizer.
///
/// yDir's dock layout costs 4 cells for the `[D] ` prefix, 11 for Size and
/// 17 for Modified, dropping the last two in that order as the width falls
/// (`Columns::adaptive`). So:
///
/// - 440 px ≈ 58 columns ≈ 56 inner: name, Size and Modified, with 24
///   cells of filename. A real file manager.
/// - 260 px ≈ 34 columns ≈ 32 inner: name and Size, 17 cells of filename.
///   Cramped but legible, which is what a minimum should be.
///
/// The old 320/200 gave 0 and 0 cells of filename under the fixed column
/// layout — the dock was reported as "too narrow to be useful" for exactly
/// this reason.
export const DOCK_MIN_WIDTH = 260;
export const DOCK_DEFAULT_WIDTH = 440;

/// Bumped when the stored width stops meaning what it used to. A width
/// written by an older ymux was clamped against a 200 px minimum and is
/// almost certainly the old, unusably narrow default, so it is replaced
/// once — deliberately overriding one earlier drag rather than leaving
/// every existing user on the width they complained about. A drag after
/// the upgrade is stored at the current version and kept from then on.
export const DOCK_STATE_VERSION = 2;

/// Which view the shared side panel shows.
export type DockView = "files" | "usage";

export interface DockState {
  open: boolean;
  width: number;
  view: DockView;
}

export function parseDockState(raw: string | null): DockState {
  const fallback: DockState = { open: false, width: DOCK_DEFAULT_WIDTH, view: "files" };
  if (!raw) return fallback;
  try {
    const v = JSON.parse(raw) as Record<string, unknown>;
    const current = v.v === DOCK_STATE_VERSION;
    const width =
      current &&
      typeof v.width === "number" &&
      Number.isFinite(v.width) &&
      v.width >= DOCK_MIN_WIDTH
        ? Math.round(v.width)
        : DOCK_DEFAULT_WIDTH;
    return { open: v.open === true, width, view: v.view === "usage" ? "usage" : "files" };
  } catch {
    return fallback;
  }
}

export function serializeDockState(s: DockState): string {
  return JSON.stringify({ ...s, v: DOCK_STATE_VERSION });
}

/// Transition for "show `view`". `toggle` is the VS Code behaviour of the
/// entry points (shortcut, bar buttons): asking for the view that is already
/// showing closes the panel. Without it the panel only ever opens.
export function nextDockState(state: DockState, view: DockView, toggle: boolean): DockState {
  if (toggle && state.open && state.view === view) return { ...state, open: false };
  return { ...state, open: true, view };
}

/// First-run state of the merged panel, from the two docks it replaces
/// (`ymux.fileDock`, `ymux.tokenDock`). The file dock's state wins; the panel
/// starts on the usage view only when that was the only dock left open.
export function migrateDockState(fileRaw: string | null, tokenRaw: string | null): DockState {
  const file = parseDockState(fileRaw);
  const token = parseDockState(tokenRaw);
  if (file.open || !token.open) return file;
  return { open: true, width: fileRaw ? file.width : token.width, view: "usage" };
}

/// Width in px, kept between DOCK_MIN_WIDTH and half of `containerWidth`.
/// The minimum wins in a window too narrow to satisfy both.
export function clampDockWidth(width: number, containerWidth: number): number {
  if (!Number.isFinite(width)) return DOCK_MIN_WIDTH;
  const max = Math.max(DOCK_MIN_WIDTH, Math.floor(containerWidth / 2));
  return Math.min(max, Math.max(DOCK_MIN_WIDTH, Math.round(width)));
}
