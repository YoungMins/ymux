/// Scrollback-restore guard.
///
/// ymux restores a pane's saved scrollback by writing it into xterm *before*
/// spawning the PTY. The problem: every ConPTY-hosted shell (powershell.exe,
/// cmd.exe, pwsh) begins its first output burst with an erase-in-display —
/// `\x1b[2J\x1b[H` (clear whole screen, cursor home). `\x1b[2J` erases the
/// *viewport* rows. If the restored history is shorter than the viewport it
/// lives entirely inside those rows, so the shell's startup clear wipes it —
/// the user sees the restored output flash in, then vanish as if `cls` ran.
///
/// Fix: after writing the restored history, emit one CRLF per viewport row.
/// That scrolls the whole restored block up out of the viewport and into the
/// scrollback ring (which `\x1b[2J` does not touch), leaving a blank viewport
/// for the shell to clear and paint its prompt into. The restored history then
/// survives, sitting in scrollback directly above the fresh prompt.
export function restoreScrollGuard(rows: number): string {
  return "\r\n".repeat(Math.max(0, rows));
}

/// How far to scroll the viewport up once the shell has painted its first
/// prompt, so the restored history is actually VISIBLE on open instead of
/// sitting silently in scrollback (which reads to the user as "nothing was
/// restored" — the screen shows only a bare prompt).
///
/// The guard above leaves the separator two lines above the viewport top
/// (one blank line, then the separator), so scrolling by `rows - 2` puts the
/// separator near the bottom of the view with the tail of the history filling
/// the rest. Typing scrolls back to the prompt on its own (xterm's
/// scroll-on-input), so this only affects what you see on open.
export function restoreRevealLines(rows: number): number {
  return Math.max(0, rows - 2);
}

/// Must the reveal wait for a real layout box?
///
/// A tab can be spawned while it is hidden (`Ctrl+Shift+T` from another tab,
/// or the file dock's viewer tab), and a `display: none` pane has no box for
/// `FitAddon` to measure — it computes NaN dimensions and returns without
/// resizing, leaving xterm at its 80×24 default. A reveal worked out there
/// scrolls by 22 lines whatever size the pane really is, so the restored
/// history lands at the wrong offset once the tab is shown and re-fitted.
///
/// So when a restore happened into an unmeasurable pane, the amount is
/// recomputed and applied at the first fit that has a box to measure
/// (`TerminalPane.scheduleFit`). A visible pane keeps the original path:
/// reveal from the real row count as soon as the shell paints.
export function shouldDeferRestoreReveal(
  restored: boolean,
  measurable: boolean,
): boolean {
  return restored && !measurable;
}
