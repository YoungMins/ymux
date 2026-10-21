import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { channelBytes } = await import("./bridge");

describe("channelBytes", () => {
  it("wraps an ArrayBuffer without copying", () => {
    const buf = new Uint8Array([27, 91, 72]).buffer;
    const out = channelBytes(buf);
    expect(Array.from(out)).toEqual([27, 91, 72]);
    expect(out.buffer).toBe(buf);
  });
  it("accepts the JSON fallback and a Uint8Array as is", () => {
    expect(Array.from(channelBytes([104, 105]))).toEqual([104, 105]);
    const u = new Uint8Array([1, 2]);
    expect(channelBytes(u)).toBe(u);
  });
});
