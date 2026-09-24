// Which folder a files pane's actions act on. Pure.
//
// `requested` is where the pane is going (the breadcrumb already shows it);
// `listed` is the folder whose entries are on screen. Between a navigation
// and its listing landing they differ, and the rows the user sees — and
// selects, renames, pastes next to — still belong to `listed`. An action
// taken then must not be aimed at `requested`: F2 on `foo.txt` from the old
// folder would *move* it into the new one. So actions get no target while a
// navigation is pending (`actionDir` is null), and a rename always builds
// its destination from the entry's own parent, never from either field.

export interface NavState {
  requested: string | null;
  listed: string | null;
}

export function initialNav(dir: string | null): NavState {
  return { requested: dir, listed: null };
}

export function navigateTo(s: NavState, dir: string): NavState {
  return { ...s, requested: dir };
}

/// A listing of `dir` arrived. Only the one that was asked for counts.
export function listingLanded(s: NavState, dir: string): NavState {
  return s.requested === dir ? { ...s, listed: dir } : s;
}

export function listingFailed(s: NavState, dir: string): NavState {
  return s.requested === dir ? { ...s, listed: null } : s;
}

/// The folder new files, folders and pastes go into, or `null` to refuse.
/// `===` here is identity, not path comparison: `listed` is only ever set
/// from the very string `requested` held.
export function actionDir(s: NavState): string | null {
  return s.listed !== null && s.listed === s.requested ? s.listed : null;
}

/// Is `b` the folder `a` already names? Both are this pane's own strings,
/// so NFC is the only folding the frontend may do (CLAUDE.md rule 15).
export function isSameDir(a: string | null, b: string | null): boolean {
  return a !== null && b !== null && a.normalize("NFC") === b.normalize("NFC");
}
