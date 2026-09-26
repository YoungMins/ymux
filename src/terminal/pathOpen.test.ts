import { describe, expect, it } from "vitest";

import { pathOpenTarget } from "./pathOpen";

const file = (absolute: string) => ({ absolute, is_dir: false });

describe("pathOpenTarget", () => {
  it("sends an existing markdown file to the in-app viewer", () => {
    expect(pathOpenTarget(file("D:\\Git\\ymux\\README.md"), true)).toBe("viewer");
    expect(pathOpenTarget(file("/srv/docs/guide.markdown"), true)).toBe("viewer");
  });

  it("matches the extension case-insensitively", () => {
    expect(pathOpenTarget(file("C:\\x\\NOTES.MD"), true)).toBe("viewer");
  });

  it("leaves a directory named like markdown to the OS", () => {
    expect(pathOpenTarget({ absolute: "/srv/book.md", is_dir: true }, true)).toBe("os");
  });

  it("leaves every other file to the OS opener and its reveal policy", () => {
    for (const p of ["a.txt", "a.ts", "a.ps1", "a.bat", "a.exe", "a.md.exe", "README", "a.mdx"]) {
      expect(pathOpenTarget(file(`C:\\x\\${p}`), true), p).toBe("os");
    }
  });

  it("falls back to the OS when there is no viewer to open in", () => {
    expect(pathOpenTarget(file("C:\\x\\README.md"), false)).toBe("os");
  });
});
