// The editor's keymap policy (spec §0.6): **ymux globals always win.**
//
// CodeMirror's keymaps `preventDefault` a key they handle, so a CM6 binding
// on a ymux chord would swallow it before main.ts's window handler could act.
// The fix is to never *bind* such a chord: `filterKeymap` drops every
// binding (and every Shift variant) whose chord is in the global set, using
// the same predicate the files pane uses (`isGlobalChord`, src/files/keys.ts),
// so the two GUI panes cannot disagree about what "global" means.
//
// Deliberately a filter, not a keydown guard. A guard that claims every
// reserved chord would also eat Ctrl+Alt+<key> on Windows — which *is*
// AltGr, the way German / Polish / … layouts type `@ { [ \ €`. Removing a
// binding costs nothing when no binding would have fired; the character
// still types.
//
// Pure: works on plain `{ key, mac, win, linux, shift, run }` objects, so the
// whole table is testable in node. The CM6 keymaps are passed in by the
// lazily loaded editor setup (`cmSetup.ts`) — importing them here would pull
// CodeMirror into the main bundle.

import { isGlobalChord, type KeyLike } from "../files/keys";

/// The subset of CM6's `KeyBinding` this policy reads. `run`/`shift` are
/// carried through untouched.
export interface BindingLike<R = unknown> {
  key?: string;
  mac?: string;
  win?: string;
  linux?: string;
  run?: R;
  shift?: R;
  scope?: string;
  preventDefault?: boolean;
  stopPropagation?: boolean;
  any?: unknown;
}

/// `ev.code` for the base keys CM6's keymaps and ymux's globals both use.
/// Only what `isGlobalChord` reads needs to be right; everything else maps
/// to "", which no global matches on `code`.
function codeFor(name: string): string {
  if (/^[a-z]$/i.test(name)) return `Key${name.toUpperCase()}`;
  if (/^[0-9]$/.test(name)) return `Digit${name}`;
  const table: Record<string, string> = {
    "[": "BracketLeft",
    "]": "BracketRight",
    "=": "Equal",
    "+": "Equal",
    "-": "Minus",
    "\\": "Backslash",
    "/": "Slash",
    ",": "Comma",
    ".": "Period",
    ";": "Semicolon",
    "'": "Quote",
    "`": "Backquote",
    " ": "Space",
  };
  return table[name] ?? "";
}

/// Parse a CM6 key name (`"Mod-Shift-z"`, `"Ctrl-Alt-["`, `"Cmd-ArrowUp"`)
/// into the event it matches on the given platform. Mirrors CM6's own
/// `normalizeKeyName`: the last `-`-separated part is the key, `Mod` is Cmd
/// on macOS and Ctrl elsewhere.
export function parseCmKey(spec: string, isMac: boolean): KeyLike {
  const parts = spec.split(/-(?!$)/);
  let name = parts[parts.length - 1];
  if (name === "Space") name = " ";
  const ev: KeyLike = {
    key: name,
    code: codeFor(name),
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
  };
  for (const mod of parts.slice(0, -1)) {
    if (/^(cmd|meta|m)$/i.test(mod)) ev.metaKey = true;
    else if (/^a(lt)?$/i.test(mod)) ev.altKey = true;
    else if (/^(c|ctrl|control)$/i.test(mod)) ev.ctrlKey = true;
    else if (/^s(hift)?$/i.test(mod)) ev.shiftKey = true;
    else if (/^mod$/i.test(mod)) {
      if (isMac) ev.metaKey = true;
      else ev.ctrlKey = true;
    }
  }
  return ev;
}

/// The chord CM6 will actually use for `b` on this platform (its own
/// platform-override rule), or `null` if the binding is inactive here.
/// ymux ships on Windows and macOS only, so `linux` never applies.
export function effectiveKey(b: BindingLike, isMac: boolean): string | null {
  if (isMac && b.mac) return b.mac;
  if (!isMac && b.win) return b.win;
  return b.key ?? null;
}

/// Selection-extending navigation keys. `isGlobalChord` reserves *every*
/// mod+Shift chord so a future global cannot be eaten by the files pane, but
/// in an editor mod+Shift+Home/End/↑/↓/PageUp/PageDown/Backspace/Delete are
/// how text gets selected and deleted, and main.ts binds none of them. They
/// are carved back out. ←/→ are *not*: mod+Shift+←/→ is ymux's swap-pane,
/// and the global wins (select-by-word stays on Shift+Option+←/→ on macOS;
/// on Windows it is lost in the editor — recorded in the step notes).
const EDITOR_NAV_KEYS = new Set([
  "Home",
  "End",
  "ArrowUp",
  "ArrowDown",
  "PageUp",
  "PageDown",
  "Backspace",
  "Delete",
]);

/// Is this chord ymux's rather than the editor's? `isGlobalChord` minus the
/// selection keys above.
export function isReservedForYmux(ev: KeyLike, isMac: boolean): boolean {
  if (!isGlobalChord(ev, isMac)) return false;
  const mod = isMac ? ev.metaKey : ev.ctrlKey;
  if (mod && ev.shiftKey && !ev.altKey && EDITOR_NAV_KEYS.has(ev.key)) return false;
  return true;
}

/// Is `spec` (a CM6 key name) a chord ymux's global handler owns?
export function isGlobalCmKey(spec: string, isMac: boolean): boolean {
  return isReservedForYmux(parseCmKey(spec, isMac), isMac);
}

/// `bindings` with every chord ymux owns removed, normalised to a single
/// `key` per binding (the platform overrides are resolved here, so a test can
/// read the table CM6 will actually see).
///
/// - A binding whose chord is global is dropped whole.
/// - A binding whose *Shift variant* is global keeps its unshifted chord and
///   loses `shift` (e.g. `Mod-g` find-next keeps Mod-g but not Mod-Shift-g).
/// - A binding with only a `linux` chord, or only a `mac` chord on Windows,
///   is inactive and dropped.
export function filterKeymap<R>(
  bindings: readonly BindingLike<R>[],
  isMac: boolean,
): BindingLike<R>[] {
  const out: BindingLike<R>[] = [];
  for (const b of bindings) {
    const key = effectiveKey(b, isMac);
    if (!key) continue;
    if (isGlobalCmKey(key, isMac)) continue;
    const next: BindingLike<R> = { ...b, key };
    delete next.mac;
    delete next.win;
    delete next.linux;
    if (next.shift !== undefined && isGlobalCmKey(`Shift-${key}`, isMac)) delete next.shift;
    out.push(next);
  }
  return out;
}

/// Every chord a filtered keymap binds, Shift variants included — the set
/// the tests check against the global predicate.
export function boundChords(bindings: readonly BindingLike[]): string[] {
  const out: string[] = [];
  for (const b of bindings) {
    if (!b.key) continue;
    out.push(b.key);
    if (b.shift !== undefined) out.push(`Shift-${b.key}`);
  }
  return out;
}
