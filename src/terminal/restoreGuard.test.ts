import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import { restoreScrollGuard, restoreRevealLines } from "./restoreGuard";

function write(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve));
}

/// The exact first output burst a ConPTY-hosted shell emits at startup,
/// captured byte-for-byte from powershell.exe and cmd.exe spawned through
/// portable-pty 0.8 (the same path ymux uses): hide cursor, clear screen,
/// SGR reset, cursor home, then the prompt is painted at the top.
const CONPTY_STARTUP_BURST = "\x1b[?25l\x1b[2J\x1b[m\x1b[HPS D:\\>\x1b[?25h";

describe("restoreScrollGuard", () => {
  it("returns one CRLF per viewport row", () => {
    expect(restoreScrollGuard(3)).toBe("\r\n\r\n\r\n");
  });

  it("keeps restored lines in scrollback when the shell's startup burst clears the screen", async () => {
    const term = new Terminal({ rows: 5, cols: 40, scrollback: 100, allowProposedApi: true });
    // Short restored history (fits entirely inside the viewport — the case
    // the ConPTY \x1b[2J would otherwise wipe completely).
    await write(term, "alpha\r\nbeta\r\ngamma\r\n-- restored --\r\n");
    await write(term, restoreScrollGuard(term.rows));
    await write(term, CONPTY_STARTUP_BURST);

    const buf = term.buffer.active;
    const lines: string[] = [];
    for (let y = 0; y < buf.length; y++) {
      lines.push(buf.getLine(y)?.translateToString(true) ?? "");
    }
    const text = lines.join("\n");
    expect(text).toContain("alpha");
    expect(text).toContain("-- restored --");
    // The prompt paints at the top of the now-blank viewport, with the
    // restored history intact in scrollback directly above it.
    expect(buf.getLine(buf.baseY)?.translateToString(true)).toContain("PS D:\\>");
    term.dispose();
  });

  it("reveals the restored history in the viewport after the reveal scroll", async () => {
    const rows = 8;
    const term = new Terminal({ rows, cols: 40, scrollback: 200, allowProposedApi: true });
    // A history longer than the viewport, so there is plenty to reveal.
    for (let i = 1; i <= 12; i++) await write(term, `line-${i}\r\n`);
    await write(term, "-- restored --\r\n");
    await write(term, restoreScrollGuard(term.rows));
    await write(term, CONPTY_STARTUP_BURST);

    const buf = term.buffer.active;
    // Before revealing, the viewport shows only the fresh prompt — this is the
    // "looks like cls ran" state the user reported.
    const viewportBefore: string[] = [];
    for (let y = buf.viewportY; y < buf.viewportY + rows; y++) {
      viewportBefore.push(buf.getLine(y)?.translateToString(true) ?? "");
    }
    expect(viewportBefore.join("\n")).not.toContain("-- restored --");

    term.scrollLines(-restoreRevealLines(rows));

    const after = term.buffer.active;
    const viewportAfter: string[] = [];
    for (let y = after.viewportY; y < after.viewportY + rows; y++) {
      viewportAfter.push(after.getLine(y)?.translateToString(true) ?? "");
    }
    const text = viewportAfter.join("\n");
    expect(text).toContain("-- restored --");
    expect(text).toContain("line-12");
    term.dispose();
  });

  it("restoreRevealLines leaves room for the separator and never goes negative", () => {
    expect(restoreRevealLines(8)).toBe(6);
    expect(restoreRevealLines(2)).toBe(0);
    expect(restoreRevealLines(1)).toBe(0);
  });

  it("without the guard the same burst erases a viewport-sized history (documents the bug)", async () => {
    const term = new Terminal({ rows: 5, cols: 40, scrollback: 100, allowProposedApi: true });
    await write(term, "alpha\r\nbeta\r\ngamma\r\n-- restored --\r\n");
    await write(term, CONPTY_STARTUP_BURST);

    const buf = term.buffer.active;
    const lines: string[] = [];
    for (let y = 0; y < buf.length; y++) {
      lines.push(buf.getLine(y)?.translateToString(true) ?? "");
    }
    expect(lines.join("\n")).not.toContain("alpha");
    term.dispose();
  });
});
