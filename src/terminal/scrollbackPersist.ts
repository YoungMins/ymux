/// Whether a pane should (re)write its scrollback to disk right now.
///
/// A pane restores its prior scrollback into the live terminal on open, so
/// `serialize()` re-captures that replayed history (plus the "session
/// restored" separator and the scroll guard). If we saved that back on every
/// open, an untouched terminal would compound its own restored history every
/// session until it hit the byte cap — the "idle prompt keeps repeating" bug.
///
/// The fix: only persist when the user actually did something this session.
/// PTYs are killed on app close, so after a restart the only way new output
/// appears is the user typing — making "typed at least once" an exact proxy
/// for "did work here". An idle pane therefore never re-saves and its prior
/// scrollback is left exactly as it was.
/// Whether an `onData` chunk represents the user actually doing something,
/// versus a terminal auto-response that reaches `onData` without any user
/// action. The latter — focus reports (DECSET 1004, which ConPTY enables at
/// startup), cursor-position replies (PSReadLine queries these constantly),
/// device-attribute replies, and mouse events — ALL begin with ESC (0x1b).
/// Real typing (printable text, Enter, Ctrl+C, Tab, Backspace) does not. So an
/// idle pane that is merely focused or clicked must not be counted as worked-in.
export function isUserActivity(data: string): boolean {
  return data.length > 0 && data.charCodeAt(0) !== 0x1b;
}

export function shouldSaveScrollback(params: {
  persistEnabled: boolean;
  hadUserActivity: boolean;
}): boolean {
  return params.persistEnabled && params.hadUserActivity;
}
