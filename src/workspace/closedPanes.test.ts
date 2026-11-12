import { describe, expect, it } from "vitest";
import { ClosedPaneStack, parsePending, type ClosedPane } from "./closedPanes";
import type { PaneSpec } from "../types";

const entry = (id: string, closedAt: number): ClosedPane => ({ spec: { id } as PaneSpec, closedAt });

describe("ClosedPaneStack", () => {
  it("pops the most recent first", () => {
    const s = new ClosedPaneStack(5, 1000);
    s.push(entry("a", 0));
    s.push(entry("b", 10));
    expect(s.pop(20)?.spec.id).toBe("b");
    expect(s.pop(20)?.spec.id).toBe("a");
    expect(s.pop(20)).toBeNull();
  });
  it("evicts the oldest beyond max and returns it for deletion", () => {
    const s = new ClosedPaneStack(2, 1000);
    expect(s.push(entry("a", 0))).toEqual([]);
    expect(s.push(entry("b", 1))).toEqual([]);
    expect(s.push(entry("c", 2)).map((e) => e.spec.id)).toEqual(["a"]);
    expect(s.ids).toEqual(["b", "c"]);
  });
  it("does not reopen an expired pane but hands it out via expire", () => {
    const s = new ClosedPaneStack(5, 1000);
    s.push(entry("a", 0));
    s.push(entry("b", 900));
    expect(s.canReopen(1000)).toBe(true);
    expect(s.pop(1000)?.spec.id).toBe("b");
    expect(s.pop(1000)).toBeNull();
    expect(s.canReopen(1000)).toBe(false);
    expect(s.expire(1000).map((e) => e.spec.id)).toEqual(["a"]);
    expect(s.size).toBe(0);
  });
  it("reports the next expiry", () => {
    const s = new ClosedPaneStack(5, 1000);
    expect(s.nextExpiry()).toBeNull();
    s.push(entry("a", 100));
    s.push(entry("b", 500));
    expect(s.nextExpiry()).toBe(1100);
  });
});

describe("parsePending", () => {
  it("keeps strings only and survives junk", () => {
    expect(parsePending(null)).toEqual([]);
    expect(parsePending("nope")).toEqual([]);
    expect(parsePending('{"a":1}')).toEqual([]);
    expect(parsePending('["x",1,"y"]')).toEqual(["x", "y"]);
  });
});
