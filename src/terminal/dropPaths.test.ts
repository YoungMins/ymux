import { describe, it, expect } from "vitest";
import { formatDroppedPaths, formatDroppedPathsChecked } from "./dropPaths";

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

describe("formatDroppedPaths for a WSL pane", () => {
  it("types the /mnt view of a drive path, POSIX-quoted", () => {
    expect(formatDroppedPaths(["C:\\a\\b c.png"], "cmd", true)).toBe("'/mnt/c/a/b c.png'");
    expect(formatDroppedPaths(["C:\\a.txt", "d:\\x\\it's.txt"], "unknown", true)).toBe(
      "'/mnt/c/a.txt' '/mnt/d/x/it'\\''s.txt'",
    );
  });

  it("leaves out and reports a path with no /mnt view", () => {
    const r = formatDroppedPathsChecked(["\\\\srv\\share\\a.png", "C:\\ok.png"], "cmd", true);
    expect(r.text).toBe("'/mnt/c/ok.png'");
    expect(r.refused).toEqual(["\\\\srv\\share\\a.png"]);
  });

  it("does not translate when the pane is not WSL", () => {
    expect(formatDroppedPaths(["C:\\a.txt"], "cmd", false)).toBe('"C:\\a.txt"');
  });

  it("reports untypeable paths for ordinary panes and ignores empty entries", () => {
    const r = formatDroppedPathsChecked(["", "/tmp/a\nb", "/tmp/c"], "posix");
    expect(r.refused).toEqual(["/tmp/a\nb"]);
    expect(r.text).toBe("'/tmp/c'");
  });
});
