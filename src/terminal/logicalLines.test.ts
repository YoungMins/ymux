import { describe, it, expect } from "vitest";
import { logicalLines } from "./logicalLines";

const row = (raw: string, wrapped = false) => ({ raw, wrapped });

describe("logicalLines", () => {
  it("joins a soft-wrapped continuation to the row before it", () => {
    expect(logicalLines([row("abcdef"), row("ghij  ", true), row("next    ")])).toEqual(["abcdefghij", "next"]);
  });

  it("joins a run of several wrapped rows", () => {
    expect(logicalLines([row("aaaa"), row("bbbb", true), row("cccc", true), row("d", true)])).toEqual(["aaaabbbbccccd"]);
  });

  it("keeps a space at the wrap point and puts a secret back together", () => {
    expect(logicalLines([row("sk-ant-api03-abc"), row("defghijkl", true)])).toEqual(["sk-ant-api03-abcdefghijkl"]);
    expect(logicalLines([row("two words "), row("here", true)])).toEqual(["two words here"]);
  });

  it("leaves unwrapped rows alone, blanks in the middle included", () => {
    expect(logicalLines([row("one  "), row("   "), row("two")])).toEqual(["one", "", "two"]);
  });

  it("drops blank lines at the end, and handles no rows", () => {
    expect(logicalLines([row("x"), row("  "), row("", false)])).toEqual(["x"]);
    expect(logicalLines([])).toEqual([]);
  });

  it("starts a line with a wrapped first row (its head scrolled out of the window)", () => {
    expect(logicalLines([row("tail of a line", true), row("fresh")])).toEqual(["tail of a line", "fresh"]);
  });
});
