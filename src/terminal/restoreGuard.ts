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
