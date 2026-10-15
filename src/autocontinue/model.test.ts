import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  LOG_LIMIT,
  applyAutoContinue,
  autoLog,
  forgetPane,
  formatEntry,
  isAutoContinue,
  onAutoContinueChange,
  recordAutoAnswer,
  resetAutoContinue,
} from "./model";

const P = "11111111-2222-3333-4444-555555555555";

beforeEach(() => resetAutoContinue());

describe("auto-continue model", () => {
  it("tracks the switch and notifies only on a real change", () => {
    const cb = vi.fn();
    const off = onAutoContinueChange(cb);
    applyAutoContinue(P, true);
    applyAutoContinue(P, true);
    expect(isAutoContinue(P)).toBe(true);
    expect(cb).toHaveBeenCalledTimes(1);
    applyAutoContinue(P, false);
    expect(isAutoContinue(P)).toBe(false);
    off();
    applyAutoContinue(P, true);
    expect(cb).toHaveBeenCalledTimes(2);
  });

  it("keeps a bounded log, newest last, and keeps it when switched off", () => {
    for (let i = 0; i < LOG_LIMIT + 3; i++) {
      recordAutoAnswer({ paneId: P, denied: false, answers: [[`q${i}`, "a"]] }, i);
    }
    expect(autoLog(P)).toHaveLength(LOG_LIMIT);
    expect(autoLog(P)[0].answers[0][0]).toBe("q3");
    applyAutoContinue(P, true);
    applyAutoContinue(P, false);
    expect(autoLog(P)).toHaveLength(LOG_LIMIT);
    forgetPane(P);
    expect(autoLog(P)).toHaveLength(0);
  });

  it("formats an answer and a denial", () => {
    const at = Date.UTC(2026, 0, 1);
    expect(formatEntry({ at, denied: false, answers: [["Q?", "A"], ["R?", "B"]] }, "no")).toContain(
      "Q? → A; R? → B",
    );
    expect(formatEntry({ at, denied: true, answers: [] }, "denied!")).toContain("denied!");
  });
});
