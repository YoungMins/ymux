import { describe, it, expect } from "vitest";
import { shouldSaveScrollback, isUserActivity } from "./scrollbackPersist";

describe("isUserActivity", () => {
  it("counts typed printable characters and Enter as activity", () => {
    expect(isUserActivity("l")).toBe(true);
    expect(isUserActivity("ls")).toBe(true);
    expect(isUserActivity("\r")).toBe(true);
    expect(isUserActivity("가")).toBe(true); // CJK input
  });

  it("counts control keystrokes that aren't escape sequences", () => {
    expect(isUserActivity("\x03")).toBe(true); // Ctrl+C
    expect(isUserActivity("\t")).toBe(true); // Tab completion
    expect(isUserActivity("\x7f")).toBe(true); // Backspace
  });

  it("does NOT count terminal auto-responses, which all start with ESC", () => {
    // These reach onData without the user doing anything: focus reporting
    // (DECSET 1004, which ConPTY enables at startup), cursor-position replies
    // (PSReadLine queries these constantly), and device-attribute replies.
    expect(isUserActivity("\x1b[I")).toBe(false); // focus in
    expect(isUserActivity("\x1b[O")).toBe(false); // focus out
    expect(isUserActivity("\x1b[24;1R")).toBe(false); // cursor position report
    expect(isUserActivity("\x1b[?1;2c")).toBe(false); // device attributes
    expect(isUserActivity("\x1b[<0;10;5M")).toBe(false); // SGR mouse event
    expect(isUserActivity("")).toBe(false);
  });
});

describe("shouldSaveScrollback", () => {
  it("saves only when persistence is on AND the user did something this session", () => {
    expect(shouldSaveScrollback({ persistEnabled: true, hadUserActivity: true })).toBe(true);
  });

  it("does NOT save an idle session, so a restored-then-untouched pane can't pile up", () => {
    // The reported bug: an idle terminal re-serialized its own restored
    // history + separator + guard every open/close, compounding to the cap.
    // Gating on real activity is what stops that.
    expect(shouldSaveScrollback({ persistEnabled: true, hadUserActivity: false })).toBe(false);
  });

  it("never saves when persistence is disabled, regardless of activity", () => {
    expect(shouldSaveScrollback({ persistEnabled: false, hadUserActivity: true })).toBe(false);
    expect(shouldSaveScrollback({ persistEnabled: false, hadUserActivity: false })).toBe(false);
  });
});
