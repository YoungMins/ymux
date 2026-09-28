// Turns clipboard text into the exact bytes a shell should see.
//
// ymux writes pastes straight to the PTY (`api.writePane`) rather than through
// `term.paste()`. That is deliberate and has to stay that way: xterm's `paste()`
// ends in `triggerDataEvent`, which would run the paste through `onData` — where
// any multi-line paste (every line ending becomes `\r`) would be mistaken for a
// submitted command by the pane status machine — and it also blanks the helper
// textarea behind `ImeBridge`'s back, stranding the IME mirror. So the framing
// xterm would have done is done here instead.
//
// What is xterm's, reproduced verbatim from `browser/Clipboard.ts` (it is an
// internal module with non-relative `browser/…` imports and is not re-exported
// from the bundle, so it cannot be imported):
//
//   * `prepareTextForTerminal` — `/\r?\n/g` → `\r`.
//   * `bracketTextForPaste` — the `ESC[200~` / `ESC[201~` markers.
//
// What is ours, because xterm does neither:
//
//   * ESC substitution. A pasted `ESC[201~` would otherwise close the bracketed
//     frame early and hand the rest of the paste to the shell as live
//     keystrokes — the classic bracketed-paste escape. Every ESC in the payload
//     becomes U+241B SYMBOL FOR ESCAPE, which is visible, inert, and the same
//     length, so nothing else shifts.
//   * Chunking, so a multi-megabyte paste does not sit in one IPC message.

import { quotePathForPaste, type ShellFamily } from "./shellQuote";

/// Start of a bracketed paste (DECSET 2004).
const PASTE_START = "\x1b[200~";
/// End of a bracketed paste.
const PASTE_END = "\x1b[201~";
/// What every ESC in the payload is replaced with: U+241B SYMBOL FOR ESCAPE.
const ESC_SYMBOL = "␛";

/// Pastes at or below this go out in one write. Matches Orca's threshold.
///
/// Thresholds here are UTF-16 code units, not encoded bytes — they exist to
/// pace the IPC, not to hit an exact byte budget, and counting code units keeps
/// the split points cheap to compute.
export const DIRECT_LIMIT = 64 * 1024;
/// Size of each piece once a paste is over `DIRECT_LIMIT`.
export const CHUNK_SIZE = 16 * 1024;

export interface PasteOptions {
  /// Whether the foreground app has enabled DECSET 2004. Read from xterm's own
  /// `term.modes.bracketedPasteMode` — the terminal already tracks it, so we
  /// never have to parse the sequence ourselves.
  bracketed: boolean;
  /// Overridable for tests. Pastes at or below this stay in one piece.
  directLimit?: number;
  /// Overridable for tests. Code units per chunk once chunking kicks in.
  chunkSize?: number;
}

/// The writes to hand the PTY, in order, for a paste of `text`.
///
/// Empty for empty input — the caller writes nothing at all, rather than an
/// empty bracketed frame.
///
/// When bracketed, the opening marker is glued to the first chunk and the
/// closing marker to the last, so no split can ever land inside a marker: the
/// markers are added after chunking, never chunked themselves.
export function preparePaste(text: string, opts: PasteOptions): string[] {
  if (!text) return [];
  const payload = sanitizePaste(text);
  const directLimit = opts.directLimit ?? DIRECT_LIMIT;
  const chunkSize = opts.chunkSize ?? CHUNK_SIZE;
  const chunks =
    payload.length <= directLimit ? [payload] : splitChunks(payload, chunkSize);
  if (!opts.bracketed) return chunks;
  chunks[0] = PASTE_START + chunks[0];
  chunks[chunks.length - 1] += PASTE_END;
  return chunks;
}

/// Line endings normalized the way a terminal wants them, and every ESC
/// defanged. Newlines first, ESC second — they cannot overlap, but the ESC pass
/// has to be the last thing that touches the payload, so that nothing added
/// afterwards (the frame markers, which legitimately contain ESC) is eaten by
/// it.
export function sanitizePaste(text: string): string {
  return text.replace(/\r?\n/g, "\r").replace(/\x1b/g, ESC_SYMBOL);
}

/// Cut `text` into pieces of at most `size` code units, never between the two
/// halves of a surrogate pair.
///
/// Each chunk is encoded to UTF-8 separately by the caller, so a split through a
/// pair would turn one astral character (an emoji, most CJK extension B) into
/// two replacement characters.
function splitChunks(text: string, size: number): string[] {
  const out: string[] = [];
  let i = 0;
  while (i < text.length) {
    let end = Math.min(i + size, text.length);
    if (end < text.length && isHighSurrogate(text.charCodeAt(end - 1))) {
      // Back off to before the pair, unless that would produce an empty chunk
      // (a chunk size of 1 landing on a pair) — then take the whole pair.
      end = end - 1 > i ? end - 1 : end + 1;
    }
    out.push(text.slice(i, end));
    i = end;
  }
  return out;
}

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

/// What a paste should do once the backend has answered "is there an image on
/// the clipboard?".
export type ImagePasteDecision =
  /// Type `write` into the PTY and stop — the clipboard held an image, which
  /// is now a file on disk.
  | { kind: "image"; write: string }
  /// No image. The caller falls through to the ordinary text paste.
  | { kind: "text" };

/// Decide from `path`, the reply of the `paste_clipboard_image` command.
///
/// `null` is the backend's "no image on the clipboard" answer. An empty or
/// blank path is treated the same way rather than trusted: the bug this whole
/// path exists to fix was a 0-byte image file whose path got typed into the
/// user's shell, so a nothing-shaped answer must never turn into a write.
///
/// The path is quoted for the pane's shell (`quotePathForPaste`): it can
/// contain spaces — a Windows profile directory like `C:\Users\John Smith\…`
/// would otherwise reach the receiving CLI as two arguments — a `$` or
/// backtick in it must never expand, and the quoting must be one Claude Code
/// can undo. A path that cannot be typed that way falls through to text as
/// well. There is no trailing newline: the user presses Enter.
export function decideImagePaste(
  path: string | null | undefined,
  family: ShellFamily,
): ImagePasteDecision {
  if (!path || !path.trim()) return { kind: "text" };
  const write = quotePathForPaste(path, family);
  if (write === null) return { kind: "text" };
  return { kind: "image", write };
}

/// Who handles a Ctrl/Cmd+V keydown in a terminal pane.
///
/// - `"ymux"`: the keydown handler cancels the key and pastes. Right on
///   Windows, where a cancelled Ctrl+V fires no `paste` event, so nothing else
///   pastes.
/// - `"native"`: the keydown is left alone and the platform's paste command
///   delivers one DOM `paste` event, which `claimPasteEvent` takes. Right on
///   macOS, where the app menu's Edit ▸ Paste owns Cmd+V: the main webview is
///   a child webview (Tauri's `unstable` feature), and wry makes a child's
///   `performKeyEquivalent:` return NO, so the menu gets the key before the
///   page and `preventDefault()` in a keydown handler cannot stop it. A keydown
///   handler that pasted as well would be one copy too many.
export function pasteKeyOwner(isMac: boolean): "ymux" | "native" {
  return isMac ? "native" : "ymux";
}

/// The parts of a DOM `ClipboardEvent` a paste needs.
export interface PasteEventLike {
  clipboardData: { getData(type: string): string } | null;
  preventDefault(): void;
  stopImmediatePropagation(): void;
}

/// Take a `paste` event aimed at the terminal: cancel it, stop it, and return
/// its plain text. All synchronous, and it has to be — `clipboardData` is
/// only readable during dispatch.
///
/// Both cancellations matter. Left alone, the event reached xterm's own
/// `paste` listener, which writes the text, and its default action inserted
/// the same text into the helper textarea, whose `input` event `ImeBridge`
/// mirrors to the PTY: the macOS Cmd+V double paste.
///
/// Reading `clipboardData` also needs no permission, where WKWebView answers
/// `navigator.clipboard.readText()` with a floating "Paste" callout.
export function claimPasteEvent(ev: PasteEventLike): string {
  ev.preventDefault();
  ev.stopImmediatePropagation();
  return ev.clipboardData?.getData("text/plain") ?? "";
}

/// Everything a terminal paste touches, injected so the order can be tested.
export interface PasteRun {
  /// `paste_clipboard_image`: the saved PNG's path, `null` for no image, a
  /// rejection when an image was there but could not be saved.
  readImage: () => Promise<string | null>;
  /// The clipboard text. Rejecting (access denied) pastes nothing.
  readText: () => Promise<string>;
  family: ShellFamily;
  /// Whether the foreground app has DECSET 2004 on, read at write time.
  bracketed: () => boolean;
  /// False until the PTY exists.
  canWrite: () => boolean;
  write: (data: string) => Promise<void>;
  reportImageError: (e: unknown) => void;
}

/// Paste the clipboard: an image as its saved file's quoted path, otherwise
/// the text, framed by `preparePaste`.
///
/// Image *before* text is deliberate: a clipboard carrying both (a cell range
/// copied out of Excel, say) pastes the image path. An image that could not be
/// saved is reported and nothing else is pasted: silently pasting the text
/// instead would be worse than nothing, and typing a path to a file that isn't
/// there is the bug the Rust image read replaced.
///
/// Chunks are awaited in order: a `void` loop would not guarantee the IPC sees
/// them in sequence, and a reordered chunk is a scrambled paste.
export async function runPaste(r: PasteRun): Promise<void> {
  try {
    const decision = decideImagePaste(await r.readImage(), r.family);
    if (decision.kind === "image") {
      if (r.canWrite()) await r.write(decision.write);
      return;
    }
  } catch (e) {
    r.reportImageError(e);
    return;
  }
  try {
    const text = await r.readText();
    if (!text || !r.canWrite()) return;
    for (const chunk of preparePaste(text, { bracketed: r.bracketed() })) {
      await r.write(chunk);
    }
  } catch {
    // Clipboard access denied — silent fail.
  }
}
