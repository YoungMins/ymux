import { describe, expect, it } from "vitest";
import { chooseTarget } from "./targetPick";

describe("chooseTarget", () => {
  it("none when empty", () => {
    expect(chooseTarget([], null)).toEqual({ kind: "none" });
  });
  it("one when there is a single id", () => {
    expect(chooseTarget(["a"], null)).toEqual({ kind: "one", id: "a" });
  });
  it("prefers the last choice if still present", () => {
    expect(chooseTarget(["a", "b"], "b")).toEqual({ kind: "one", id: "b" });
  });
  it("ignores a last choice that is gone", () => {
    expect(chooseTarget(["a", "b"], "z")).toEqual({ kind: "many", ids: ["a", "b"] });
  });
  it("many when ambiguous", () => {
    expect(chooseTarget(["a", "b"], undefined)).toEqual({ kind: "many", ids: ["a", "b"] });
  });
});
