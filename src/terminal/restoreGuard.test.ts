import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import { restoreScrollGuard } from "./restoreGuard";

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
