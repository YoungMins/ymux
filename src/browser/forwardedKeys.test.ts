import { describe, it, expect } from "vitest";
import { forwardedKeyInit } from "./forwardedKeys";

const p = (code: string, ctrl: boolean, shift: boolean, alt: boolean, key = "") => ({
  key,
  code,
  ctrl,
  shift,
  alt,
});

describe("forwardedKeyInit", () => {
  it("derives the key from the code, ignoring the payload's key", () => {
    expect(forwardedKeyInit(p("KeyH", true, true, false, "W"))).toMatchObject({
      key: "H",
      code: "KeyH",
      ctrlKey: true,
      shiftKey: true,
      altKey: false,
    });
    expect(forwardedKeyInit(p("Tab", true, false, false, "F"))?.key).toBe("Tab");
    expect(forwardedKeyInit(p("Digit3", true, false, true))?.key).toBe("3");
    expect(forwardedKeyInit(p("KeyN", true, false, true))?.key).toBe("n");
    expect(forwardedKeyInit(p("BracketRight", true, true, false))?.key).toBe("}");
    expect(forwardedKeyInit(p("Tab", true, true, false))?.key).toBe("Tab");
  });

  it("never forwards close-pane", () => {
    expect(forwardedKeyInit(p("KeyW", true, true, false, "W"))).toBeNull();
  });

  it("refuses anything outside the table", () => {
    for (const bad of [
      p("KeyA", true, true, false),
      p("KeyH", true, false, false),
      p("KeyH", true, true, true),
      p("Tab", true, false, true),
      p("Tab", false, false, false),
      p("Digit0", true, false, true),
      p("KeyF", true, false, false, "F"),
    ]) {
      expect(forwardedKeyInit(bad)).toBeNull();
    }
  });

  it("refuses a malformed payload", () => {
    expect(forwardedKeyInit(null)).toBeNull();
    expect(forwardedKeyInit({ code: 5, ctrl: true, shift: true, alt: false })).toBeNull();
    expect(forwardedKeyInit({ code: "KeyH", ctrl: "yes", shift: true, alt: false })).toBeNull();
  });
});
