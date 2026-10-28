// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { followPointer, swallowNextClick } from "./pointerDrag";

const pointer = (type: string): Event => new MouseEvent(type, { clientX: 5, clientY: 6 });

describe("followPointer", () => {
  it("follows moves until the release, then stops listening", () => {
    const move = vi.fn();
    const end = vi.fn();
    followPointer(move, end);
    window.dispatchEvent(pointer("pointermove"));
    window.dispatchEvent(pointer("pointerup"));
    window.dispatchEvent(pointer("pointermove"));
    expect(move).toHaveBeenCalledTimes(1);
    expect(end).toHaveBeenCalledTimes(1);
    expect(end.mock.calls[0][0]).not.toBeNull();
  });

  it("ends with null when cancelled, once", () => {
    const end = vi.fn();
    const cancel = followPointer(() => {}, end);
    window.dispatchEvent(new Event("blur"));
    cancel();
    window.dispatchEvent(pointer("pointerup"));
    expect(end.mock.calls).toEqual([[null]]);
  });
});

describe("swallowNextClick", () => {
  it("eats only a click that comes right away", async () => {
    const seen = vi.fn();
    document.body.addEventListener("click", seen);
    swallowNextClick();
    document.body.click();
    expect(seen).not.toHaveBeenCalled();
    swallowNextClick();
    await new Promise((r) => setTimeout(r, 5));
    document.body.click();
    expect(seen).toHaveBeenCalledTimes(1);
  });
});
