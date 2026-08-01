import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import { resyncNudge } from "./viewportSync";

function write(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve));
}

/// A terminal with `lines` rows of history above an 8-row viewport, i.e. a
/// non-zero `baseY` — the only situation where the scrollbar can drift.
async function scrolledTerminal(lines = 40): Promise<Terminal> {
  const term = new Terminal({ rows: 8, cols: 40, scrollback: 200, allowProposedApi: true });
  for (let i = 1; i <= lines; i++) await write(term, `line-${i}\r\n`);
  return term;
}

describe("resyncNudge", () => {
  it("does nothing when there is no scrollback to drift from", () => {
    expect(resyncNudge(0, 0, 0)).toEqual([]);
  });

  it("does nothing when the DOM scrollbar survived the layout change", () => {
    // Any non-zero scrollTop means the layout box was never destroyed — a
    // reset always lands on exactly 0.
    expect(resyncNudge(30, 50, 620)).toEqual([]);
  });

  it("does nothing when the buffer really is at the top of the scrollback", () => {
    // scrollTop 0 is the correct value here, not a stale one.
    expect(resyncNudge(0, 50, 0)).toEqual([]);
  });

  it("nudges when a reset scrollbar disagrees with a scrolled-up buffer", () => {
    expect(resyncNudge(30, 50, 0)).toEqual([-1, 1]);
  });

  it("nudges when the buffer sits at the bottom but the scrollbar reset", () => {
    expect(resyncNudge(50, 50, 0)).toEqual([-1, 1]);
  });

  it("assumes staleness when the viewport element could not be read", () => {
    expect(resyncNudge(30, 50, null)).toEqual([-1, 1]);
    expect(resyncNudge(0, 0, null)).toEqual([]);
  });
});

describe("applying the nudge to a real buffer", () => {
  it("leaves the visible viewport exactly where it was, from the bottom", async () => {
    const term = await scrolledTerminal();
    const before = term.buffer.active.viewportY;
    expect(before).toBe(term.buffer.active.baseY);

    for (const step of resyncNudge(before, term.buffer.active.baseY, 0)) {
      term.scrollLines(step);
    }

    expect(term.buffer.active.viewportY).toBe(before);
    term.dispose();
  });

  it("leaves the visible viewport exactly where it was, scrolled up", async () => {
    const term = await scrolledTerminal();
    term.scrollLines(-10);
    const before = term.buffer.active.viewportY;
    expect(before).toBeGreaterThan(0);

    for (const step of resyncNudge(before, term.buffer.active.baseY, 0)) {
      term.scrollLines(step);
    }

    expect(term.buffer.active.viewportY).toBe(before);
    term.dispose();
  });

  it("each step really moves, so xterm gets the scroll events it syncs on", async () => {
    const term = await scrolledTerminal();
    term.scrollLines(-10);
    const buf = term.buffer.active;
    const steps = resyncNudge(buf.viewportY, buf.baseY, 0);
    expect(steps.length).toBe(2);

    const start = buf.viewportY;
    term.scrollLines(steps[0]);
    // A no-op move is swallowed by xterm without firing onScroll, which is
    // what drives its scrollbar re-sync — so the first step must move.
    expect(term.buffer.active.viewportY).not.toBe(start);
    term.scrollLines(steps[1]);
    expect(term.buffer.active.viewportY).toBe(start);
    term.dispose();
  });

  it("emits no nudge for a terminal that never filled its viewport", async () => {
    const term = new Terminal({ rows: 8, cols: 40, scrollback: 200, allowProposedApi: true });
    await write(term, "alpha\r\nbeta\r\n");
    const buf = term.buffer.active;
    expect(buf.baseY).toBe(0);
    expect(resyncNudge(buf.viewportY, buf.baseY, 0)).toEqual([]);
    term.dispose();
  });
});
