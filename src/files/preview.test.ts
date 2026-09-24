import { describe, it, expect } from "vitest";
import {
  BINARY_SNIFF_BYTES,
  MAX_PREVIEW_BYTES,
  MAX_PREVIEW_ENTRIES,
  MAX_PREVIEW_LINES,
  completeUtf8Length,
  decodePreview,
  directoryPreview,
  isProbablyBinary,
} from "./preview";

const enc = new TextEncoder();
const bytes = (s: string) => enc.encode(s);

describe("decodePreview", () => {
  it("returns the head of a text file as lines", () => {
    const p = decodePreview(bytes("fn main() {\n    println!(\"hi\");\n}\n"), 33);
    expect(p).toEqual({
      kind: "text",
      lines: ["fn main() {", '    println!("hi");', "}"],
      truncated: false,
    });
  });

  it("strips CR from CRLF line endings", () => {
    const p = decodePreview(bytes("a\r\nb\r\n"), 6);
    expect(p).toEqual({ kind: "text", lines: ["a", "b"], truncated: false });
  });

  it("keeps a last line that has no trailing newline", () => {
    const p = decodePreview(bytes("one\ntwo"), 7);
    expect(p).toEqual({ kind: "text", lines: ["one", "two"], truncated: false });
  });

  it("shows an empty file as no lines", () => {
    expect(decodePreview(new Uint8Array(0), 0)).toEqual({
      kind: "text",
      lines: [],
      truncated: false,
    });
  });

  it("caps the line count and says it did", () => {
    const text = Array.from({ length: MAX_PREVIEW_LINES + 50 }, (_, i) => `line ${i}`).join("\n");
    const p = decodePreview(bytes(text), text.length);
    expect(p.kind).toBe("text");
    if (p.kind !== "text") return;
    expect(p.lines).toHaveLength(MAX_PREVIEW_LINES);
    expect(p.truncated).toBe(true);
  });

  it("reports a read that stopped at the byte cap as truncated", () => {
    const head = bytes("x".repeat(MAX_PREVIEW_BYTES));
    const p = decodePreview(head, MAX_PREVIEW_BYTES * 3);
    expect(p.kind === "text" && p.truncated).toBe(true);
  });

  it("drops a Hangul syllable the byte cap cut in half instead of rendering U+FFFD", () => {
    const full = bytes("가나다");
    // "다" is 3 bytes; cut after its first byte.
    const cut = full.slice(0, full.length - 2);
    const p = decodePreview(cut, full.length);
    expect(p).toEqual({ kind: "text", lines: ["가나"], truncated: true });
  });

  it("decodes a genuinely invalid byte mid-file lossily rather than cutting there", () => {
    const b = new Uint8Array([0x61, 0xff, 0x62, 0x0a]);
    const p = decodePreview(b, 4);
    expect(p).toEqual({ kind: "text", lines: ["a�b"], truncated: false });
  });

  it("calls a file with a NUL in its head binary", () => {
    expect(decodePreview(new Uint8Array([0x50, 0x4b, 0x00, 0x03]), 4)).toEqual({
      kind: "binary",
    });
  });
});

describe("isProbablyBinary", () => {
  it("only looks at the sniff window, the same one the backend uses", () => {
    const late = new Uint8Array(BINARY_SNIFF_BYTES + 10).fill(0x61);
    late[BINARY_SNIFF_BYTES + 5] = 0;
    expect(isProbablyBinary(late)).toBe(false);
    late[BINARY_SNIFF_BYTES - 1] = 0;
    expect(isProbablyBinary(late)).toBe(true);
  });
  it("does not call Hangul text binary", () => {
    expect(isProbablyBinary(bytes("한글 텍스트"))).toBe(false);
  });
});

describe("completeUtf8Length", () => {
  it("is the full length for complete text", () => {
    expect(completeUtf8Length(bytes("abc한"))).toBe(6);
  });
  it("backs off an incomplete 2-, 3- and 4-byte tail", () => {
    const e = bytes("é"); // 2 bytes
    expect(completeUtf8Length(e.slice(0, 1))).toBe(0);
    const h = bytes("a한"); // 1 + 3
    expect(completeUtf8Length(h.slice(0, 3))).toBe(1);
    const emoji = bytes("a\u{1F600}"); // 1 + 4
    expect(completeUtf8Length(emoji.slice(0, 4))).toBe(1);
  });
  it("leaves a stray continuation byte alone (it is invalid, not incomplete)", () => {
    expect(completeUtf8Length(new Uint8Array([0x61, 0x80]))).toBe(2);
  });
});

describe("directoryPreview", () => {
  it("orders like the listing and marks directories", () => {
    const p = directoryPreview(
      [
        { name: "b.txt", is_dir: false },
        { name: "src", is_dir: true },
        { name: "A.md", is_dir: false },
      ],
      false,
    );
    expect(p).toEqual({
      kind: "dir",
      entries: [
        { name: "src", is_dir: true },
        { name: "A.md", is_dir: false },
        { name: "b.txt", is_dir: false },
      ],
      more: false,
    });
  });

  it("caps the entries and says there are more", () => {
    const many = Array.from({ length: MAX_PREVIEW_ENTRIES + 5 }, (_, i) => ({
      name: `f${String(i).padStart(4, "0")}`,
      is_dir: false,
    }));
    const p = directoryPreview(many, false);
    expect(p.kind === "dir" && p.entries.length).toBe(MAX_PREVIEW_ENTRIES);
    expect(p.kind === "dir" && p.more).toBe(true);
  });

  it("passes on a scan the backend stopped early", () => {
    const p = directoryPreview([{ name: "a", is_dir: false }], true);
    expect(p.kind === "dir" && p.more).toBe(true);
  });

  it("an empty directory is an empty list, not an error", () => {
    expect(directoryPreview([], false)).toEqual({ kind: "dir", entries: [], more: false });
  });
});
