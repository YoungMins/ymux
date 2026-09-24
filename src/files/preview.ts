// Pure preview model for the files pane: given the head of a file (bytes) or
// the head of a directory (names), decide what the preview says. The caps
// and the UTF-8 rule are ported from the retired ydir TUI's preview so the
// pane and the TUI it replaced make the same decisions:
//
//  - read at most 64 KiB, keep at most 200 lines, name at most 200 entries;
//  - a NUL in the first 8 KiB means binary — the same window as the backend's
//    `fsx::is_probably_binary`, so the preview and "open" never disagree;
//  - a read cap that cuts a multi-byte character (a Hangul syllable is 3
//    bytes) drops the fragment instead of rendering U+FFFD, while a genuinely
//    invalid byte *inside* the text is decoded lossily rather than hiding
//    everything after it.
//
// What is no longer ported: ydir's control-character `sanitize`. It existed so
// a file full of escape sequences could not repaint a terminal; the pane
// renders through `textContent`, which cannot be repainted.

import { compareCodePoints } from "./fileModel";

/// Bytes the backend reads for a preview (`fs_read_head`'s cap).
export const MAX_PREVIEW_BYTES = 64 * 1024;
export const MAX_PREVIEW_LINES = 200;
export const MAX_PREVIEW_ENTRIES = 200;
/// Must equal `fsx::BINARY_SNIFF_BYTES`.
export const BINARY_SNIFF_BYTES = 8 * 1024;

export interface PreviewDirEntry {
  name: string;
  is_dir: boolean;
}

export type Preview =
  | { kind: "text"; lines: string[]; truncated: boolean }
  | { kind: "binary" }
  | { kind: "dir"; entries: PreviewDirEntry[]; more: boolean };

export function isProbablyBinary(head: Uint8Array): boolean {
  const n = Math.min(head.length, BINARY_SNIFF_BYTES);
  for (let i = 0; i < n; i++) if (head[i] === 0) return true;
  return false;
}

/// Length of `b` with an *incomplete* trailing UTF-8 sequence removed. A
/// stray continuation byte with no lead byte in reach is invalid, not
/// incomplete, and is left for the lossy decoder.
export function completeUtf8Length(b: Uint8Array): number {
  const len = b.length;
  for (let back = 1; back <= Math.min(4, len); back++) {
    const byte = b[len - back];
    if ((byte & 0xc0) === 0x80) continue; // continuation: keep looking
    let need = 1;
    if ((byte & 0xe0) === 0xc0) need = 2;
    else if ((byte & 0xf0) === 0xe0) need = 3;
    else if ((byte & 0xf8) === 0xf0) need = 4;
    return need > back ? len - back : len;
  }
  return len;
}

/// Preview of a file whose first `head.length` bytes are `head`, out of
/// `totalSize` bytes on disk.
export function decodePreview(head: Uint8Array, totalSize: number): Preview {
  if (isProbablyBinary(head)) return { kind: "binary" };
  const end = completeUtf8Length(head);
  const text = new TextDecoder("utf-8", { fatal: false }).decode(head.subarray(0, end));
  // `str::lines()` semantics: split on \n, strip one trailing \r, and no
  // empty last line for a file that ends in a newline.
  const raw = text.split("\n");
  if (raw.length && raw[raw.length - 1] === "") raw.pop();
  const lines = raw.map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l));
  const truncated =
    totalSize > head.length || end < head.length || lines.length > MAX_PREVIEW_LINES;
  return { kind: "text", lines: lines.slice(0, MAX_PREVIEW_LINES), truncated };
}

/// Preview of a directory from a bounded scan of its entries. `scanStopped`
/// is the backend saying it did not reach the end.
export function directoryPreview(
  entries: readonly PreviewDirEntry[],
  scanStopped: boolean,
): Preview {
  const sorted = [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return compareCodePoints(a.name.toLowerCase(), b.name.toLowerCase());
  });
  return {
    kind: "dir",
    entries: sorted.slice(0, MAX_PREVIEW_ENTRIES),
    more: scanStopped || sorted.length > MAX_PREVIEW_ENTRIES,
  };
}
