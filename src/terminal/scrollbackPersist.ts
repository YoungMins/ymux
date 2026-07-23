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
export function shouldSaveScrollback(params: {
  persistEnabled: boolean;
  hadUserActivity: boolean;
}): boolean {
  return params.persistEnabled && params.hadUserActivity;
}
