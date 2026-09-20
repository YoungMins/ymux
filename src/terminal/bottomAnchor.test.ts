import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import {
  anchorOffset,
  anchorTransform,
  bufferAnchorOffset,
  lastContentRow,
  type AnchorBuffer,
} from "./bottomAnchor";

function write(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve));
}

function headless(rows = 10): Terminal {
  return new Terminal({ rows, cols: 40, scrollback: 200, allowProposedApi: true });
}

describe("anchorOffset", () => {
  const base = { rows: 10, cursorY: 0, lastContentRow: -1, altBuffer: false, atBottom: true };

  it("pushes a lone prompt on an empty screen down to the last row", () => {
    expect(anchorOffset(base)).toBe(9);
  });

  it("is 0 when the cursor is already on the last row", () => {
    expect(anchorOffset({ ...base, cursorY: 9 })).toBe(0);
  });

  it("is 0 in the alternate buffer", () => {
    expect(anchorOffset({ ...base, altBuffer: true })).toBe(0);
  });

  it("is 0 while the user has scrolled up", () => {
    expect(anchorOffset({ ...base, atBottom: false })).toBe(0);
  });

  it("anchors the lowest content row when content sits below the cursor", () => {
    expect(anchorOffset({ ...base, cursorY: 2, lastContentRow: 6 })).toBe(3);
  });

  it("anchors the cursor row when it is below the last content", () => {
    expect(anchorOffset({ ...base, cursorY: 5, lastContentRow: 3 })).toBe(4);
  });

  it("is 0 for a single-row terminal", () => {
    expect(anchorOffset({ ...base, rows: 1 })).toBe(0);
  });

  it("is never negative", () => {
    expect(anchorOffset({ ...base, cursorY: 12 })).toBe(0);
  });
});

describe("bufferAnchorOffset on a real xterm buffer", () => {
  it("anchors a fresh prompt to the bottom", async () => {
    const term = headless(10);
    await write(term, "$ ");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(9);
    term.dispose();
  });

  it("follows output as it grows", async () => {
    const term = headless(10);
    await write(term, "one\r\ntwo\r\nthree\r\n$ ");
    // Prompt on viewport row 3 -> 10 - 1 - 3.
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(6);
    term.dispose();
  });

  it("is 0 once the screen is full", async () => {
    const term = headless(5);
    for (let i = 0; i < 12; i++) await write(term, `line-${i}\r\n`);
    await write(term, "$ ");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });

  it("anchors the prompt again after an ED2 clear that leaves scrollback (baseY > 0)", async () => {
    const term = headless(10);
    for (let i = 0; i < 40; i++) await write(term, `line-${i}\r\n`);
    // PowerShell `cls` / ConPTY: erase display + home, scrollback kept.
    await write(term, "\x1b[2J\x1b[HPS D:\> ");
    const buf = term.buffer.active;
    expect(buf.baseY).toBeGreaterThan(0);
    expect(buf.viewportY).toBe(buf.baseY);
    expect(buf.cursorY).toBe(0);
    expect(bufferAnchorOffset(buf, term.rows)).toBe(9);
    term.dispose();
  });

  it("reads content rows in the viewport frame, not the absolute frame", async () => {
    const term = headless(10);
    for (let i = 0; i < 40; i++) await write(term, `line-${i}\r\n`);
    await write(term, "\x1b[2J\x1b[H");
    // Content on viewport rows 0..5, then park the cursor on row 1.
    await write(term, "a\r\nb\r\nc\r\nd\r\ne\r\nf\x1b[2;1H");
    const buf = term.buffer.active;
    expect(buf.cursorY).toBe(1);
    // Lowest content row is viewport row 5 -> 10 - 1 - 5.
    expect(bufferAnchorOffset(buf, term.rows)).toBe(4);
    term.dispose();
  });

  it("is 0 while scrolled up", async () => {
    const term = headless(5);
    for (let i = 0; i < 20; i++) await write(term, `line-${i}\r\n`);
    await write(term, "\x1b[2J\x1b[H$ ");
    term.scrollLines(-1);
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });

  it("is 0 in the alternate buffer", async () => {
    const term = headless(10);
    await write(term, "$ vim\r\n\x1b[?1049h\x1b[H~");
    expect(term.buffer.active.type).toBe("alternate");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });
});

/// A fake buffer that counts `getLine` calls, to pin the scan-cost bound.
function countingBuffer(
  over: Partial<AnchorBuffer>,
  contentRows: ReadonlySet<number> = new Set(),
): { buf: AnchorBuffer; calls: () => number } {
  let calls = 0;
  const baseY = over.baseY ?? 0;
  const buf: AnchorBuffer = {
    type: "normal",
    baseY,
    viewportY: baseY,
    cursorY: 0,
    ...over,
    getLine(y: number) {
      calls++;
      return { translateToString: () => (contentRows.has(y - baseY) ? "x" : "") };
    },
  };
  return { buf, calls: () => calls };
}

describe("scan cost", () => {
  it("never scans in the alternate buffer", () => {
    const { buf, calls } = countingBuffer({ type: "alternate" });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("never scans while scrolled up", () => {
    const { buf, calls } = countingBuffer({ baseY: 100, viewportY: 90 });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("never scans when the cursor is on the last row", () => {
    const { buf, calls } = countingBuffer({ cursorY: 49 });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("scans only the rows below the cursor", () => {
    const { buf, calls } = countingBuffer({ baseY: 100, cursorY: 10 });
    expect(bufferAnchorOffset(buf, 50)).toBe(39);
    expect(calls()).toBe(39); // rows 49..11
  });

  it("stops at the first non-empty row from the bottom", () => {
    const { buf, calls } = countingBuffer({ cursorY: 0 }, new Set([49]));
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(1);
  });
});

describe("lastContentRow", () => {
  it("returns -1 when nothing below the floor has content", () => {
    const { buf } = countingBuffer({}, new Set([2]));
    expect(lastContentRow(buf, 10, 2)).toBe(-1);
  });

  it("treats a missing line as blank", () => {
    const buf: AnchorBuffer = {
      type: "normal", baseY: 0, viewportY: 0, cursorY: 0,
      getLine: () => undefined,
    };
    expect(lastContentRow(buf, 10, 0)).toBe(-1);
  });
});

describe("anchorTransform", () => {
  it("is empty for a zero offset", () => {
    expect(anchorTransform(0, 200, 10)).toBe("");
  });

  it("converts rows to CSS pixels via the screen height", () => {
    expect(anchorTransform(3, 200, 10)).toBe("translateY(60px)");
  });

  it("keeps fractional cell heights exact", () => {
    expect(anchorTransform(2, 170, 10)).toBe("translateY(34px)");
    expect(anchorTransform(1, 175, 10)).toBe("translateY(17.5px)");
  });

  it("is empty before the renderer has sized the screen", () => {
    expect(anchorTransform(3, Number.NaN, 10)).toBe("");
    expect(anchorTransform(3, 0, 10)).toBe("");
    expect(anchorTransform(3, 200, 0)).toBe("");
  });
});
