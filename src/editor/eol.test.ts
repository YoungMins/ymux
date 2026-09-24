import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { detectEol, eolLabel, needsEolWarning, normalizeToLf, restoreEol, saveEol } from "./eol";

// Mirrors src-tauri/src/textfile.rs's tests, then goes one step further than
// Rust can: through CodeMirror's own document, which is what actually sits
// between the read and the write.

describe("detectEol (mirrors textfile::detect_eol)", () => {
  it("detects every ending", () => {
    expect(detectEol("a\nb\n")).toBe("lf");
    expect(detectEol("a\r\nb\r\n")).toBe("crlf");
    expect(detectEol("a\rb\r")).toBe("cr");
    expect(detectEol("a\r\nb\n")).toBe("mixed");
    expect(detectEol("a\rb\n")).toBe("mixed");
    expect(detectEol("one line")).toBe("none");
    expect(detectEol("")).toBe("none");
  });

  it("does not count a CRLF's halves as a lone CR and LF", () => {
    expect(detectEol("x\r\n")).toBe("crlf");
  });
});

describe("saveEol / restoreEol", () => {
  it("writes mixed back as LF, and warns first", () => {
    expect(saveEol("mixed")).toBe("lf");
    expect(needsEolWarning("mixed")).toBe(true);
    expect(needsEolWarning("crlf")).toBe(false);
    expect(restoreEol("a\nb", "mixed")).toBe("a\nb");
  });

  it("labels", () => {
    expect(eolLabel("crlf")).toBe("CRLF");
    expect(eolLabel("none")).toBe("LF");
  });
});

/// raw file text → what fs_read_text returns → CodeMirror doc → what the
/// editor sends to fs_write_text → the bytes restoreEol writes.
function throughEditor(raw: string): string {
  const eol = detectEol(raw);
  const state = EditorState.create({ doc: normalizeToLf(raw) });
  return restoreEol(state.doc.toString(), eol);
}

describe("an untouched file saves byte-for-byte through the editor's document", () => {
  const cases: [string, string][] = [
    ["CRLF with a trailing newline", "fn main() {\r\n    println!(\"안녕\");\r\n}\r\n"],
    ["CRLF without a trailing newline", "a\r\nb"],
    ["LF", "line 1\nline 2\n"],
    ["LF, no trailing newline", "line 1\nline 2"],
    ["CR only", "a\rb\r"],
    ["blank lines at the end", "x\r\n\r\n\r\n"],
    ["a single line", "hello"],
    ["empty", ""],
    ["Hangul, emoji and a tab", "\t가나다 😀\r\n라마\r\n"],
  ];
  for (const [name, raw] of cases) {
    it(name, () => {
      expect(throughEditor(raw)).toBe(raw);
    });
  }

  it("a mixed file comes back LF (the one deliberate rewrite)", () => {
    expect(throughEditor("a\r\nb\nc\r")).toBe("a\nb\nc\n");
  });

  it("a CRLF paste into an LF buffer does not leave a stray CR", () => {
    let state = EditorState.create({ doc: "a\n" });
    state = state.update({ changes: { from: 1, insert: "\r\nb" } }).state;
    expect(state.doc.toString()).toBe("a\nb\n");
  });
});
