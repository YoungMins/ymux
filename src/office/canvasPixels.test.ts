import { describe, expect, it } from "vitest";
import { writeRgba } from "./canvasPixels";

describe("writeRgba", () => {
  it("writes colours and leaves null pixels transparent", () => {
    const data = new Uint8ClampedArray(2 * 1 * 4);
    writeRgba(data, ["#ff8000", null], 2);
    expect([...data]).toEqual([255, 128, 0, 255, 0, 0, 0, 0]);
  });

  it("places a frame at a column of a wider sheet", () => {
    const data = new Uint8ClampedArray(4 * 2 * 4);
    writeRgba(data, ["#010203", "#040506", "#070809", "#0a0b0c"], 2, 4, 2);
    const px = (x: number, y: number) => [...data.slice((y * 4 + x) * 4, (y * 4 + x) * 4 + 4)];
    expect(px(0, 0)).toEqual([0, 0, 0, 0]);
    expect(px(2, 0)).toEqual([1, 2, 3, 255]);
    expect(px(3, 1)).toEqual([10, 11, 12, 255]);
  });
});
