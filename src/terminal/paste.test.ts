import { describe, it, expect } from "vitest";
import {
  CHUNK_SIZE,
  DIRECT_LIMIT,
  preparePaste,
  sanitizePaste,
} from "./paste";

const START = "\x1b[200~";
const END = "\x1b[201~";
const ESC_SYMBOL = "␛";

/// What the PTY ends up with, for the cases where only the whole matters.
function joined(chunks: string[]): string {
  return chunks.join("");
}

describe("sanitizePaste", () => {
  it("turns CRLF into a bare CR", () => {
    expect(sanitizePaste("a\r\nb")).toBe("a\rb");
  });

  it("turns a lone LF into a CR", () => {
    expect(sanitizePaste("a\nb\nc")).toBe("a\rb\rc");
  });

  it("leaves an existing bare CR alone", () => {
    expect(sanitizePaste("a\rb")).toBe("a\rb");
  });

  it("replaces every ESC with the escape symbol", () => {
    expect(sanitizePaste("\x1b[31mred\x1b[0m")).toBe(
      `${ESC_SYMBOL}[31mred${ESC_SYMBOL}[0m`,
    );
  });

  it("keeps the payload the same length when defanging ESC", () => {
    const text = "\x1b\x1b\x1b";
    expect(sanitizePaste(text)).toHaveLength(text.length);
  });
});

describe("preparePaste", () => {
  it("writes nothing at all for empty input", () => {
    expect(preparePaste("", { bracketed: true })).toEqual([]);
    expect(preparePaste("", { bracketed: false })).toEqual([]);
  });

  it("sends plain text unwrapped when the app has not enabled DECSET 2004", () => {
    expect(preparePaste("ls -la", { bracketed: false })).toEqual(["ls -la"]);
  });

  it("wraps in the bracketed-paste frame when the app has enabled it", () => {
    expect(preparePaste("ls -la", { bracketed: true })).toEqual([
      `${START}ls -la${END}`,
    ]);
  });

  it("normalizes newlines inside the frame", () => {
    expect(preparePaste("one\r\ntwo\nthree", { bracketed: true })).toEqual([
      `${START}one\rtwo\rthree${END}`,
    ]);
  });

  it("defangs a pasted end marker so it cannot close the frame early", () => {
    // The attack: clipboard text carrying its own `ESC[201~`. Left intact, the
    // shell would treat everything after it as typed keystrokes and run
    // `rm -rf /` without the user pressing Enter.
    const evil = `safe${END}rm -rf /\n`;
    const [only] = preparePaste(evil, { bracketed: true });
    expect(only).toBe(`${START}safe${ESC_SYMBOL}[201~rm -rf /\r${END}`);
    // Exactly one opener and one closer survive: our own.
    expect(only.split(START)).toHaveLength(2);
    expect(only.split(END)).toHaveLength(2);
  });

  it("defangs escapes even when the frame is off", () => {
    expect(preparePaste("\x1b]0;title\x07", { bracketed: false })).toEqual([
      `${ESC_SYMBOL}]0;title\x07`,
    ]);
  });

  it("keeps a paste at the direct limit in a single write", () => {
    const text = "x".repeat(DIRECT_LIMIT);
    expect(preparePaste(text, { bracketed: false })).toHaveLength(1);
  });

  it("chunks a paste past the direct limit", () => {
    const text = "x".repeat(DIRECT_LIMIT + 1);
    const chunks = preparePaste(text, { bracketed: false });
    expect(chunks.length).toBe(Math.ceil((DIRECT_LIMIT + 1) / CHUNK_SIZE));
    expect(joined(chunks)).toBe(text);
  });

  it("preserves order and content across chunks", () => {
    const text = "abcdefghij";
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 4,
      chunkSize: 3,
    });
    expect(chunks).toEqual(["abc", "def", "ghi", "j"]);
  });

  it("never splits the frame markers across chunks", () => {
    const text = "abcdefghij";
    const chunks = preparePaste(text, {
      bracketed: true,
      directLimit: 4,
      chunkSize: 3,
    });
    // The opener rides entirely on the first chunk, the closer entirely on the
    // last, because the markers are added after the payload is cut.
    expect(chunks[0]).toBe(`${START}abc`);
    expect(chunks[chunks.length - 1]).toBe(`j${END}`);
    expect(chunks.filter((c) => c.includes(START))).toHaveLength(1);
    expect(chunks.filter((c) => c.includes(END))).toHaveLength(1);
    expect(joined(chunks)).toBe(`${START}${text}${END}`);
  });

  it("does not cut between the halves of a surrogate pair", () => {
    // Each chunk is UTF-8 encoded on its own, so a split through the pair
    // would ship two replacement characters instead of the emoji.
    const text = `abc\u{1F600}def`;
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 1,
      chunkSize: 4,
    });
    for (const chunk of chunks) {
      expect(chunk).not.toMatch(/[\uD800-\uDBFF]$/);
      expect(chunk).not.toMatch(/^[\uDC00-\uDFFF]/);
    }
    expect(joined(chunks)).toBe(text);
  });

  it("keeps a surrogate pair whole even when a chunk can hold only one unit", () => {
    const text = `\u{1F600}a`;
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 1,
      chunkSize: 1,
    });
    expect(chunks).toEqual(["\u{1F600}", "a"]);
  });

  it("still frames a paste that had to be chunked", () => {
    const text = "y".repeat(DIRECT_LIMIT + 10);
    const chunks = preparePaste(text, { bracketed: true });
    expect(joined(chunks)).toBe(`${START}${text}${END}`);
  });
});
