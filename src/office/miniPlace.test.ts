import { describe, expect, it } from "vitest";
import { clampMini, DEFAULT_MINI, MAX_SCALE, MIN_SCALE, movedMini, resizedMini } from "./miniPlace";

describe("miniPlace", () => {
  it("keeps the mini office inside the window", () => {
    expect(movedMini(DEFAULT_MINI, -5000, -5000, 1600, 900)).toEqual({ scale: 0.5, right: 1600 - 480, bottom: 900 - 300 });
    expect(movedMini(DEFAULT_MINI, 100, 100, 1600, 900)).toEqual({ scale: 0.5, right: 0, bottom: 0 });
    expect(movedMini(DEFAULT_MINI, -20, -30, 1600, 900)).toEqual({ scale: 0.5, right: 32, bottom: 42 });
  });

  it("resizes from the top-left corner within the scale range", () => {
    expect(resizedMini(DEFAULT_MINI, -96, 0, 1600, 900).scale).toBeCloseTo(0.6);
    expect(resizedMini(DEFAULT_MINI, 0, -60, 1600, 900).scale).toBeCloseTo(0.6);
    expect(resizedMini(DEFAULT_MINI, 2000, 2000, 1600, 900).scale).toBe(MIN_SCALE);
    expect(resizedMini(DEFAULT_MINI, -2000, 0, 1600, 900).scale).toBe(MAX_SCALE);
  });

  it("repairs a broken saved place", () => {
    expect(clampMini({ right: NaN, bottom: -4, scale: NaN }, 1600, 900)).toEqual({ right: 12, bottom: 0, scale: 0.5 });
    // A window smaller than the mini office pins it to the corner.
    expect(clampMini({ right: 50, bottom: 50, scale: 0.75 }, 300, 200)).toEqual({ right: 0, bottom: 0, scale: 0.75 });
  });
});
