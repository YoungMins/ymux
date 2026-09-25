import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import {
  restoreScrollGuard,
  restoreGuardTail,
  restoreRevealLines,
  shouldDeferRestoreReveal,
} from "./restoreGuard";

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

  it("homes the cursor after the guard only where ConPTY does not", () => {
    expect(restoreGuardTail(true)).toBe("");
    expect(restoreGuardTail(false)).toBe("\x1b[H");
  });

  it("without ConPTY, the prompt lands at the viewport top and the reveal shows history + prompt, as on Windows", async () => {
    const rows = 8;
    // What a macOS zsh prints first: no clear, just the prompt.
    const ZSH_PROMPT = "me@mac ~ % ";
    const run = async (conpty: boolean): Promise<{ promptRow: number; view: string[] }> => {
      const term = new Terminal({ rows, cols: 40, scrollback: 200, allowProposedApi: true });
      for (let i = 1; i <= 12; i++) await write(term, `line-${i}\r\n`);
      await write(term, "-- restored --\r\n");
      await write(term, restoreScrollGuard(term.rows));
      await write(term, restoreGuardTail(conpty));
      await write(term, conpty ? CONPTY_STARTUP_BURST : ZSH_PROMPT);
      const needle = conpty ? "PS D:\\>" : ZSH_PROMPT.trim();
      const buf = term.buffer.active;
      const promptRow = buf.cursorY;
      expect(buf.getLine(buf.baseY + promptRow)?.translateToString(true)).toContain(needle);
      term.scrollLines(-restoreRevealLines(rows));
      const view: string[] = [];
      for (let y = buf.viewportY; y < buf.viewportY + rows; y++) {
        view.push(buf.getLine(y)?.translateToString(true) ?? "");
      }
      term.dispose();
      return { promptRow, view: view.map((l) => l.replace(needle, "<prompt>")) };
    };
    const win = await run(true);
    const mac = await run(false);
    expect(mac.promptRow).toBe(0);
    expect(mac.promptRow).toBe(win.promptRow);
    // Same picture on open: history tail, separator, prompt.
    expect(mac.view.map((l) => l.trim())).toEqual(win.view.map((l) => l.trim()));
    expect(mac.view.join("\n")).toContain("-- restored --");
    expect(mac.view.join("\n")).toContain("<prompt>");
  });

  it("restoreRevealLines leaves room for the separator and never goes negative", () => {
    expect(restoreRevealLines(8)).toBe(6);
    expect(restoreRevealLines(2)).toBe(0);
    expect(restoreRevealLines(1)).toBe(0);
  });

  it("defers the reveal only for a restore into a pane with no layout box", () => {
    // A tab spawned while hidden (`Ctrl+Shift+T` while another tab is shown,
    // or the dock's viewer tab) has no layout box, so `fit()` leaves xterm at
    // its 80×24 default and a reveal computed now would be 22 lines — wrong
    // for the size the pane actually gets when it is shown.
    expect(shouldDeferRestoreReveal(true, false)).toBe(true);
    // Visible pane: reveal from the real row count, as before.
    expect(shouldDeferRestoreReveal(true, true)).toBe(false);
    // Nothing was restored — there is nothing to reveal either way.
    expect(shouldDeferRestoreReveal(false, false)).toBe(false);
    expect(shouldDeferRestoreReveal(false, true)).toBe(false);
  });

  it("a deferred reveal lands correctly when the pane is shown at another size", async () => {
    // The hidden-tab case end to end: the guard is written at xterm's 24-row
    // default because the pane had no layout box, the shell's burst lands,
    // and only then is the tab shown and fitted — here to 40 rows.
    const guardRows = 24;
    const shownRows = 40;
    const term = new Terminal({
      rows: guardRows,
      cols: 40,
      scrollback: 500,
      allowProposedApi: true,
    });
    for (let i = 1; i <= 60; i++) await write(term, `line-${i}\r\n`);
    await write(term, "-- restored --\r\n");
    await write(term, restoreScrollGuard(guardRows));
    await write(term, CONPTY_STARTUP_BURST);
    term.resize(40, shownRows);

    const viewport = (): string[] => {
      const buf = term.buffer.active;
      const out: string[] = [];
      for (let y = buf.viewportY; y < buf.viewportY + shownRows; y++) {
        out.push(buf.getLine(y)?.translateToString(true) ?? "");
      }
      return out;
    };
    const rowOf = (lines: string[], needle: string): number =>
      lines.findIndex((l) => l.includes(needle));

    // Growing the viewport does not pull the parked history back into view:
    // the prompt is still alone at the top. Something must scroll.
    expect(rowOf(viewport(), "-- restored --")).toBe(-1);

    // The reveal is keyed to the rows the pane is *shown* at, not the rows
    // the guard was written with. Measured with both: at `shownRows` the
    // separator lands at row 36 of 40 with the prompt just under it, which is
    // the intended "history fills the view, prompt at the bottom"; at
    // `guardRows` (22 lines) the prompt stops mid-screen at row 22 and the
    // bottom of the view is empty.
    term.scrollLines(-restoreRevealLines(shownRows));
    const shown = viewport();
    expect(rowOf(shown, "-- restored --")).toBeGreaterThan(shownRows - 6);
    expect(rowOf(shown, "PS D:\\>")).toBeGreaterThan(shownRows - 4);
    expect(shown.join("\n")).toContain("line-60");
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
