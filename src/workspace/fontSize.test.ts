import { describe, it, expect } from "vitest";
import { clampFontSize, MIN_FONT_SIZE, MAX_FONT_SIZE } from "./fontSize";

// The clamp is what stands between the zoom shortcuts and a terminal the user
// cannot recover from: xterm's `fit()` divides the container by the glyph box,
// so a size at or near zero produces an absurd column count, and a huge one
// leaves no rows at all. Held-down Ctrl+- makes that one keystroke away.
describe("clampFontSize", () => {
  it("passes ordinary sizes through", () => {
    expect(clampFontSize(13)).toBe(13);
    expect(clampFontSize(20)).toBe(20);
  });

  it("clamps to the documented bounds", () => {
    expect(clampFontSize(0)).toBe(MIN_FONT_SIZE);
    expect(clampFontSize(-40)).toBe(MIN_FONT_SIZE);
    expect(clampFontSize(200)).toBe(MAX_FONT_SIZE);
  });

  it("keeps the bounds themselves", () => {
    expect(clampFontSize(MIN_FONT_SIZE)).toBe(MIN_FONT_SIZE);
    expect(clampFontSize(MAX_FONT_SIZE)).toBe(MAX_FONT_SIZE);
  });

  it("rounds fractional sizes", () => {
    expect(clampFontSize(13.4)).toBe(13);
    expect(clampFontSize(13.6)).toBe(14);
  });

  it("falls back to the default on a non-finite value", () => {
    // `Number("")` is 0 and `Number("abc")` is NaN — both reachable from the
    // Settings number input, which hands over whatever was typed.
    expect(clampFontSize(Number.NaN)).toBe(13);
    expect(clampFontSize(Number.POSITIVE_INFINITY)).toBe(13);
  });
});
