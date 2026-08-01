/// Scrollbar/buffer re-sync after a layout rebuild.
///
/// xterm.js drives its viewport from a *real* scrollable `<div>`
/// (`.xterm-viewport`): a wheel notch adds pixels to that element's
/// `scrollTop`, and the resulting native `scroll` event is translated back into
/// a buffer row (`Math.round(scrollTop / rowHeight)`). The DOM scrollbar is
/// therefore the authority for where the wheel lands.
///
/// The browser silently resets `scrollTop` to 0 whenever an element's layout
/// box is destroyed — which ymux does routinely:
///   * `SplitContainer.render()` detaches and re-appends every pane element on
///     each layout rebuild (split, close, move, zoom toggle, rename, …),
///   * `WorkspaceManager` hides inactive workspaces with `display: none`,
///   * `.workspace--zoomed` hides every non-zoomed pane the same way.
///
/// xterm keeps its own `ydisp` across all of that and never notices the drift:
/// `Viewport.syncScrollArea()` only runs off a buffer scroll or a resize, and
/// an idle pane has neither. So the next single wheel notch is applied to a
/// `scrollTop` of 0, maps back to buffer row ~0, and the view teleports to the
/// very top of the scrollback.
///
/// Fix: nudge the buffer one line and straight back. Each nudge fires the
/// scroll event that makes xterm re-run its own scrollbar sync (using its own
/// exact row height), and the pair is a no-op for the visible view and for
/// xterm's `isUserScrolling` state.
///
/// Returns the `Terminal.scrollLines()` arguments to apply, in order — empty
/// when the scrollbar is already in step with the buffer.
///
/// @param viewportY  `buffer.active.viewportY` — first buffer row on screen.
/// @param baseY      `buffer.active.baseY` — first row when scrolled to bottom.
/// @param scrollTop  `.xterm-viewport`'s DOM scroll offset, or `null` when it
///                   could not be read (then staleness is assumed).
export function resyncNudge(
  viewportY: number,
  baseY: number,
  scrollTop: number | null,
): readonly number[] {
  // Nothing has scrolled out of view yet, so a scrollTop of 0 is the truth.
  if (baseY === 0) return [];
  // A destroyed layout box always resets to exactly 0. Any other value means
  // the scrollbar survived and still matches the buffer.
  if (scrollTop !== null && scrollTop !== 0) return [];
  // The buffer genuinely sits at the top of the scrollback — 0 is correct.
  if (viewportY === 0) return [];
  // Up one line, then back down. `scrollLines` ignores a no-op move, so the
  // order matters: from any `viewportY > 0` both steps really move.
  return [-1, 1];
}
