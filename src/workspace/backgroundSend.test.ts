import { describe, expect, it } from "vitest";
import { shouldFocusAfterSend } from "./backgroundSend";

describe("shouldFocusAfterSend", () => {
  it("focuses inside the active workspace", () => expect(shouldFocusAfterSend(undefined, 1, 1)).toBe(true));
  it("stays put for another workspace", () => expect(shouldFocusAfterSend(undefined, 2, 1)).toBe(false));
  it("an explicit choice wins", () => {
    expect(shouldFocusAfterSend(true, 2, 1)).toBe(true);
    expect(shouldFocusAfterSend(false, 1, 1)).toBe(false);
  });
});
