// Mirrors the xterm helper textarea into the PTY, so IME text survives.
//
// The bug this exists for: on macOS (WKWebView, which is what Tauri gives us)
// the Hangul IME never fires a single composition event. It edits the helper
// textarea directly, through `input` events, and dispatches the `keydown`
// *after* the edit with `keyCode` 229 — "the IME already ate this key".
// Typing `안녕하세요` produces, per keystroke:
//
//     input insertText            "ㅇ"   textarea="ㅇ"
//     input insertReplacementText "아"   textarea="아"
//     input insertReplacementText "안"   textarea="안"
//     input insertText            "ㄴ"   textarea="안ㄴ"
//     input insertReplacementText "녀"   textarea="안녀"
//     …
//
// A syllable is built by *replacing* the character in place. xterm's
// `_inputEvent` only forwards `inputType === "insertText"`, so every one of
// those replacements is dropped on the floor and only the leading jamo of each
// syllable reaches the shell — `안녕하세요` arrives as `ㅇㄴㅎ세요`. Its other
// hook, `CompositionHelper.keydown`'s 229 branch, diffs the textarea across a
// `setTimeout(…, 0)` and would have caught this — except the edit has already
// landed by the time that keydown fires, so it always diffs a string against
// itself and sends nothing.
//
// The textarea, though, is always exactly right: it is the IME's own edit
// buffer. So we stop reading `inputType` and mirror the buffer instead. On
// every `input` we diff the new value against what we have already sent,
// retract the characters that changed with DEL, and send their replacements.
// The shell's line editor does the rest, and the fix is inputType-agnostic —
// dictation and drag-dropped text ride the same path.
//
// Composition events are still handled, for the platforms that do fire them
// (WebView2 on Windows). There `compositionend.data` is authoritative and the
// mirror stands down until the composition closes.
//
// Interception works because xterm binds its `input` / composition listeners
// on the helper `<textarea>` itself. Events dispatched at that textarea reach
// it in the AT_TARGET phase, where the capture flag is ignored and listeners
// fire in registration order — so we cannot outrun xterm there. On any
// *ancestor*, a capture-phase listener runs strictly before the target's, and
// `stopImmediatePropagation()` from it means xterm's handlers never run at
// all. Hence `root` below must be an ancestor of the textarea, not the
// textarea.

/// DEL. What a terminal sends for Backspace, and what we send to retract a
/// character the IME has since revised.
const DEL = "\x7f";

/// The bits of the xterm DOM + terminal this bridge needs. Kept as a plain
/// interface so the logic is testable without a DOM or a real Terminal.
export interface ImeHost {
  /// An ancestor of the helper textarea — xterm's `.xterm` element. Capture
  /// listeners here beat xterm's own AT_TARGET ones.
  root: ImeEventTarget;
  /// xterm's hidden helper textarea: the IME's edit buffer, and the thing we
  /// mirror.
  textarea: ImeTextarea;
  /// xterm's `.composition-view` div, used as the pre-commit preview on
  /// platforms that fire composition events. Null-tolerant: a headless or
  /// partial terminal simply gets no preview.
  view: ImeCompositionView | null;
  /// Terminal font, mirrored onto the preview so it lines up with the cells.
  font: () => { family: string; size: number };
  /// Deliver text to the terminal as user input.
  send: (data: string) => void;
}

export interface ImeEventTarget {
  addEventListener(
    type: string,
    listener: ImeListener,
    options?: { capture?: boolean },
  ): void;
  removeEventListener(
    type: string,
    listener: ImeListener,
    options?: { capture?: boolean },
  ): void;
}

export type ImeListener = (ev: ImeEvent) => void;

export interface ImeTextarea {
  value: string;
  readonly style: { left: string; top: string; height: string };
}

export interface ImeCompositionView {
  textContent: string | null;
  readonly style: {
    left: string;
    top: string;
    height: string;
    lineHeight: string;
    fontFamily: string;
    fontSize: string;
  };
  readonly classList: { toggle(token: string, force: boolean): void };
}

/// Minimal shape of the events we consume. `data` is the composed string on
/// `compositionupdate` / `compositionend`; `input` carries it too, but we
/// deliberately ignore it in favour of the textarea.
export interface ImeEvent {
  data?: string | null;
  inputType?: string;
  stopImmediatePropagation(): void;
}

/// The keydown fields that tell us who owns a keystroke.
export interface ImeKeyEvent {
  isComposing?: boolean;
  keyCode?: number;
  key?: string;
}

/// Keys that carry no text and must not end an IME run: pressing Shift to
/// reach `ㄲ` mid-syllable would otherwise drop the buffer on the floor.
const MODIFIER_KEYCODES = new Set([16, 17, 18, 20, 91, 92, 93, 224]);

/// True when this keydown belongs to the IME rather than to the terminal.
///
/// Three signals, because no single one is portable: `isComposing` is the
/// spec's answer but is false on the keystroke that opens a composition — and
/// false throughout under WKWebView, which fires no composition events at all;
/// `keyCode === 229` is the legacy marker every engine sets for a key the IME
/// consumed; `key === "Process"` is what older WebKit reports instead.
export function isCompositionKey(ev: ImeKeyEvent): boolean {
  return ev.isComposing === true || ev.keyCode === 229 || ev.key === "Process";
}

/// Number of leading characters `a` and `b` share.
///
/// Deliberately over code units rather than code points: the textarea is
/// indexed in code units, and a mismatch inside a surrogate pair still yields
/// a prefix ending on a boundary we can safely retract from — that surrogate
/// half's own DEL erases the whole character in the terminal.
export function commonPrefixLength(a: string, b: string): number {
  const max = Math.min(a.length, b.length);
  let i = 0;
  while (i < max && a[i] === b[i]) i++;
  return i;
}

/// What to send to the PTY to turn `mirrored` into `next`: one DEL per
/// character dropped from the tail, then whatever replaced it. Empty when the
/// buffer did not change.
export function mirrorEdit(mirrored: string, next: string): string {
  const keep = commonPrefixLength(mirrored, next);
  return DEL.repeat(mirrored.length - keep) + next.slice(keep);
}

export class ImeBridge {
  /// True between `compositionstart` and `compositionend`, on the platforms
  /// that fire them. Always false under WKWebView.
  private composing = false;
  /// The textarea content already reflected in the PTY. The diff base.
  private mirrored = "";
  private cleanups: Array<() => void> = [];

  constructor(private readonly host: ImeHost) {}

  get isComposing(): boolean {
    return this.composing;
  }

  install(): void {
    this.on("compositionstart", (ev) => {
      ev.stopImmediatePropagation();
      this.composing = true;
      this.paint("");
    });
    this.on("compositionupdate", (ev) => {
      ev.stopImmediatePropagation();
      this.paint(ev.data ?? "");
    });
    this.on("compositionend", (ev) => {
      ev.stopImmediatePropagation();
      this.composing = false;
      this.paint("");
      const data = ev.data ?? "";
      // The composition path owns its whole run, so the buffer it leaves
      // behind is already accounted for — resync rather than re-send it.
      this.reset();
      if (data) this.host.send(data);
    });
    // Every `input` on the helper textarea is IME, dictation or dropped text:
    // xterm cancels ordinary keydowns before the character can ever reach the
    // textarea, and handles paste on its own `paste` listener. So there is no
    // plain-typing path here to double up with.
    this.on("input", (ev) => {
      ev.stopImmediatePropagation();
      // While a real composition is open its own `compositionend` delivers the
      // text; mirroring the half-built syllable too would double it.
      if (this.composing) return;
      this.flush();
    });
  }

  /// Send whatever the textarea has gained or lost since the last mirror.
  private flush(): void {
    const next = this.host.textarea.value;
    const edit = mirrorEdit(this.mirrored, next);
    this.mirrored = next;
    if (edit) this.host.send(edit);
  }

  /// Called for every keydown, before xterm sees it. Returns true when the key
  /// belongs to the IME and xterm must keep its hands off it.
  ///
  /// A key that is *not* the IME's ends the run: xterm is about to send an
  /// Enter, an arrow, a control byte — after which the shell's line buffer no
  /// longer corresponds to anything in the textarea, and a later diff against
  /// a stale mirror would spray backspaces at whatever came next.
  handleKeyDown(ev: ImeKeyEvent): boolean {
    if (isCompositionKey(ev)) return true;
    if (ev.keyCode !== undefined && MODIFIER_KEYCODES.has(ev.keyCode)) {
      return false;
    }
    this.reset();
    return false;
  }

  /// Drop the mirror and the buffer behind it, so the next run starts clean.
  private reset(): void {
    this.mirrored = "";
    this.host.textarea.value = "";
  }

  dispose(): void {
    for (const off of this.cleanups) off();
    this.cleanups = [];
    this.composing = false;
    this.mirrored = "";
  }

  private on(type: string, listener: ImeListener): void {
    this.host.root.addEventListener(type, listener, { capture: true });
    this.cleanups.push(() =>
      this.host.root.removeEventListener(type, listener, { capture: true }),
    );
  }

  /// Show `text` as the pre-commit preview, or hide it when empty. Only ever
  /// used on platforms that fire composition events — where the text is held
  /// back from the terminal until it commits, and so needs somewhere to live.
  ///
  /// xterm keeps the helper textarea parked on the cursor cell (`_syncTextArea`
  /// runs on every render and, with its composition helper never activating,
  /// never stops running). Borrowing those coordinates is what keeps the
  /// preview on the caret without reaching into xterm's private render service.
  private paint(text: string): void {
    const view = this.host.view;
    if (!view) return;
    view.textContent = text;
    const { style: ta } = this.host.textarea;
    const font = this.host.font();
    view.style.left = ta.left;
    view.style.top = ta.top;
    view.style.height = ta.height;
    view.style.lineHeight = ta.height;
    view.style.fontFamily = font.family;
    view.style.fontSize = `${font.size}px`;
    view.classList.toggle("active", text.length > 0);
  }
}
