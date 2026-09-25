import { describe, it, expect } from "vitest";
import { formatDroppedPaths } from "./dropPaths";

describe("formatDroppedPaths", () => {
  it("double-quotes a single path for cmd so spaces survive as one argument", () => {
    expect(formatDroppedPaths(["C:\\Users\\a b\\notes.txt"], "cmd")).toBe(
      '"C:\\Users\\a b\\notes.txt"',
    );
  });

  it("joins multiple paths with a space, each quoted", () => {
    expect(formatDroppedPaths(["C:\\a.txt", "D:\\b c.png"], "cmd")).toBe(
      '"C:\\a.txt" "D:\\b c.png"',
    );
  });

  it("quotes for the pane's shell so nothing in a name expands", () => {
    expect(formatDroppedPaths(["/Users/me/$HOME `id`/it's.txt"], "posix")).toBe(
      "'/Users/me/$HOME `id`/it'\\''s.txt'",
    );
    expect(formatDroppedPaths(["C:\\$(calc)\\it's.txt"], "powershell")).toBe(
      "'C:\\$(calc)\\it''s.txt'",
    );
  });

  it("returns an empty string for no paths", () => {
    expect(formatDroppedPaths([], "posix")).toBe("");
  });

  it("skips empty entries rather than emitting bare quotes", () => {
    expect(formatDroppedPaths(["", "C:\\a.txt", ""], "cmd")).toBe('"C:\\a.txt"');
  });

  it("leaves out a path whose name holds a newline instead of typing Enter", () => {
    expect(formatDroppedPaths(["/tmp/a\nrm -rf ~", "/tmp/b"], "posix")).toBe("'/tmp/b'");
  });
});
