/// Terminal font-size bounds and clamping.
///
/// Its own module rather than a corner of WorkspaceManager so it can be unit
/// tested: importing the manager pulls in xterm and its addons, which need a
/// browser environment the test runner does not provide.

/// Fallback when no size is configured. Mirrors `default_font_size()` in
/// `src-tauri/src/config/model.rs`.
export const DEFAULT_FONT_SIZE = 13;

/// Mirrors MIN_FONT_SIZE / MAX_FONT_SIZE in the Rust model. Below ~6px
/// xterm's glyph metrics collapse and `fit()` computes absurd column counts;
/// above ~40px a pane can no longer hold a usable terminal.
export const MIN_FONT_SIZE = 6;
export const MAX_FONT_SIZE = 40;

export function clampFontSize(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_FONT_SIZE;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(px)));
}
