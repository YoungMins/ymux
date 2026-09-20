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
/// The path is quoted because it can contain spaces — a Windows profile
/// directory like `C:\Users\John Smith\…` would otherwise reach the receiving
/// CLI as two arguments. There is no trailing newline: the user presses Enter.
export function decideImagePaste(
  path: string | null | undefined,
): ImagePasteDecision {
  if (!path || !path.trim()) return { kind: "text" };
  return { kind: "image", write: `"${path}"` };
}
