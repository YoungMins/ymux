/// The one monospace font stack for everything ymux draws in a fixed-width
/// face: terminals, the editor, and the monospace bits of the chrome.
///
/// Windows fonts come first (Cascadia, then Consolas, which every Windows
/// ships). macOS has neither, and the generic `monospace` resolves to Courier
/// in WKWebView, so SF Mono / Menlo (shipped with every macOS) come next;
/// Courier New and the generic keyword are only the last resort.
///
/// `style.css` carries the same list as `--font-mono`; `fonts.test.ts` keeps
/// the two in step. xterm needs the literal string (it measures the font on a
/// canvas, where `var(...)` means nothing), so TS uses this constant.
export const MONO_FONT_NAMES =
  '"Cascadia Code", "Cascadia Mono", Consolas, "SF Mono", Menlo, Monaco';

export const MONO_FONT_STACK = `${MONO_FONT_NAMES}, ui-monospace, "Courier New", monospace`;
