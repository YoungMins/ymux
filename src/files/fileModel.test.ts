import { describe, it, expect } from "vitest";
import {
  applyHidden,
  baseName,
  crumbs,
  extendTo,
  findConflict,
  formatSize,
  joinPath,
  moveCursor,
  nextSelectionAfterDelete,
  parentPath,
  reconcile,
  resolveOverwrite,
  selectAll,
  selectOnly,
  sortEntries,
  stemEnd,
  targetNames,
  toggleAt,
  typeAhead,
  uniqueName,
  type FileEntry,
} from "./fileModel";

function f(name: string, is_dir = false, size = 0): FileEntry {
  return { name, path: `/d/${name}`, is_dir, is_symlink: false, size, modified_ms: 0 };
}

const names = (es: FileEntry[]) => es.map((e) => e.name);

describe("sortEntries", () => {
  it("puts directories first, then names case-insensitively (ydir's order)", () => {
    const out = sortEntries([f("b.txt"), f("Zeta", true), f("A.txt"), f("alpha", true)]);
    expect(names(out)).toEqual(["alpha", "Zeta", "A.txt", "b.txt"]);
  });

  it("sorts Hangul names in syllable (code point) order after ASCII", () => {
    const out = sortEntries([f("하나.md"), f("가나.md"), f("zeta.md"), f("나비.md")]);
    expect(names(out)).toEqual(["zeta.md", "가나.md", "나비.md", "하나.md"]);
  });

  it("compares by code point, matching Rust's String order, not UTF-16 units", () => {
    // U+FF21 (fullwidth A, BMP) vs U+1F600 (astral). UTF-16 compares the
    // surrogate 0xD83D below 0xFF21; code point order puts the emoji last.
    const out = sortEntries([f("\u{1F600}"), f("\uFF41")]);
    expect(names(out)).toEqual(["\uFF41", "\u{1F600}"]);
  });

  it("does not mutate its input", () => {
    const input = [f("b"), f("a")];
    sortEntries(input);
    expect(names(input)).toEqual(["b", "a"]);
  });
});

describe("applyHidden", () => {
  it("drops dotfiles unless showing hidden", () => {
    const es = [f(".git", true), f("src", true), f(".env"), f("a")];
    expect(names(applyHidden(es, false))).toEqual(["src", "a"]);
    expect(applyHidden(es, true)).toHaveLength(4);
  });
});

describe("selection", () => {
  const list = ["a", "b", "c", "d", "e"];

  it("selectOnly selects one row and anchors there", () => {
    const s = selectOnly(list, 2);
    expect(s.cursor).toBe(2);
    expect(s.anchor).toBe(2);
    expect([...s.selected]).toEqual(["c"]);
  });

  it("extendTo selects the anchor..index range in either direction", () => {
    const s = extendTo(selectOnly(list, 1), list, 3);
    expect(targetNames(s, list)).toEqual(["b", "c", "d"]);
    expect(s.anchor).toBe(1);
    expect(s.cursor).toBe(3);
    const back = extendTo(s, list, 0);
    expect(targetNames(back, list)).toEqual(["a", "b"]);
  });

  it("toggleAt adds and removes a row and re-anchors", () => {
    let s = toggleAt(selectOnly(list, 0), list, 2);
    expect(targetNames(s, list)).toEqual(["a", "c"]);
    expect(s.anchor).toBe(2);
    s = toggleAt(s, list, 0);
    expect(targetNames(s, list)).toEqual(["c"]);
  });

  it("moveCursor clamps, and extends from the anchor with shift", () => {
    let s = moveCursor(selectOnly(list, 0), list, -1, false);
    expect(s.cursor).toBe(0);
    s = moveCursor(s, list, 2, true);
    expect(targetNames(s, list)).toEqual(["a", "b", "c"]);
    s = moveCursor(s, list, 99, false);
    expect(s.cursor).toBe(4);
    expect(targetNames(s, list)).toEqual(["e"]);
  });

  it("selectAll selects everything, keeping the cursor", () => {
    const s = selectAll(selectOnly(list, 3), list);
    expect(targetNames(s, list)).toEqual(list);
    expect(s.cursor).toBe(3);
  });

  it("targetNames falls back to the cursor row when nothing is selected", () => {
    const s = toggleAt(selectOnly(list, 1), list, 1);
    expect(s.selected.size).toBe(0);
    expect(targetNames(s, list)).toEqual(["b"]);
    expect(targetNames(selectOnly([], 0), [])).toEqual([]);
  });

  it("reconcile keeps the selection by name across a refresh", () => {
    const s = extendTo(selectOnly(list, 1), list, 2); // b, c; cursor on c
    const next = ["0", "a", "b", "c", "e"]; // "0" arrived, "d" left
    const r = reconcile(s, list, next);
    expect(targetNames(r, next)).toEqual(["b", "c"]);
    expect(r.cursor).toBe(3);
  });

  it("reconcile clamps the cursor when its row disappeared", () => {
    const s = selectOnly(list, 4);
    const r = reconcile(s, list, ["a", "b"]);
    expect(r.cursor).toBe(1);
    expect(r.selected.size).toBe(0);
  });

  it("reconcile can put the cursor on a preferred name (the dir we came up from)", () => {
    const r = reconcile(selectOnly(["x"], 0), ["x"], ["a", "src", "z"], "src");
    expect(r.cursor).toBe(1);
    expect(targetNames(r, ["a", "src", "z"])).toEqual(["src"]);
  });
});

describe("nextSelectionAfterDelete", () => {
  const list = ["a", "b", "c", "d"];
  it("picks the next surviving row at or after the cursor", () => {
    expect(nextSelectionAfterDelete(list, new Set(["b", "c"]), 1)).toBe("d");
  });
  it("falls back to the row before when the last rows went", () => {
    expect(nextSelectionAfterDelete(list, new Set(["c", "d"]), 3)).toBe("b");
  });
  it("is null when everything went", () => {
    expect(nextSelectionAfterDelete(list, new Set(list), 0)).toBeNull();
  });
});

describe("overwrite resolution", () => {
  const taken = ["foo.txt", "foo (2).txt", "Dir", "v1.2", ".env", "archive.tar.gz"];

  it("uniqueName numbers before the extension and skips taken numbers", () => {
    expect(uniqueName("foo.txt", false, taken)).toBe("foo (3).txt");
    expect(uniqueName("bar.txt", false, taken)).toBe("bar.txt");
  });

  it("uniqueName treats directories and dotfiles as having no extension", () => {
    expect(uniqueName("Dir", true, taken)).toBe("Dir (2)");
    expect(uniqueName("v1.2", true, taken)).toBe("v1.2 (2)");
    expect(uniqueName(".env", false, taken)).toBe(".env (2)");
    expect(uniqueName("archive.tar.gz", false, taken)).toBe("archive.tar (2).gz");
  });

  it("uniqueName avoids case-only collisions (case-insensitive volumes)", () => {
    expect(uniqueName("FOO.txt", false, taken)).toBe("FOO (3).txt");
  });

  it("findConflict matches case- and composition-insensitively", () => {
    const es = [f("README.md"), f("\u1112\u1161\u11ab.txt")]; // NFD 한
    expect(findConflict("readme.md", es)?.name).toBe("README.md");
    expect(findConflict("\uD55C.txt", es)?.name).toBe("\u1112\u1161\u11ab.txt");
    expect(findConflict("other", es)).toBeUndefined();
  });

  it("writes straight through when nothing is in the way", () => {
    expect(resolveOverwrite("a.txt", false, undefined, "skip", taken)).toEqual({
      action: "write",
      name: "a.txt",
      overwrite: false,
    });
  });

  it("replace overwrites file onto file", () => {
    expect(resolveOverwrite("foo.txt", false, f("foo.txt"), "replace", taken)).toEqual({
      action: "write",
      name: "foo.txt",
      overwrite: true,
    });
  });

  it("replace is never applied to a directory, either side: it keeps both", () => {
    expect(resolveOverwrite("Dir", true, f("Dir", true), "replace", taken)).toEqual({
      action: "write",
      name: "Dir (2)",
      overwrite: false,
    });
    expect(resolveOverwrite("Dir", false, f("Dir", true), "replace", taken)).toEqual({
      action: "write",
      name: "Dir (2)",
      overwrite: false,
    });
  });

  it("keep-both renames and skip skips", () => {
    expect(resolveOverwrite("foo.txt", false, f("foo.txt"), "keep-both", taken)).toEqual({
      action: "write",
      name: "foo (3).txt",
      overwrite: false,
    });
    expect(resolveOverwrite("foo.txt", false, f("foo.txt"), "skip", taken)).toEqual({
      action: "skip",
    });
  });
});

describe("paths", () => {
  it("joins with the path's own separator", () => {
    expect(joinPath("C:\\Users", "me")).toBe("C:\\Users\\me");
    expect(joinPath("C:\\", "me")).toBe("C:\\me");
    expect(joinPath("/home", "me")).toBe("/home/me");
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("\\\\wsl.localhost\\Ubuntu\\home", "me")).toBe(
      "\\\\wsl.localhost\\Ubuntu\\home\\me",
    );
  });

  it("parentPath stops at the root", () => {
    expect(parentPath("C:\\Users\\me")).toBe("C:\\Users");
    expect(parentPath("C:\\Users")).toBe("C:\\");
    expect(parentPath("C:\\")).toBeNull();
    expect(parentPath("/home/me/")).toBe("/home");
    expect(parentPath("/home")).toBe("/");
    expect(parentPath("/")).toBeNull();
    expect(parentPath("\\\\server\\share\\dir")).toBe("\\\\server\\share\\");
    expect(parentPath("\\\\server\\share\\")).toBeNull();
  });

  it("baseName returns the last component", () => {
    expect(baseName("C:\\Users\\문서")).toBe("문서");
    expect(baseName("/a/b/")).toBe("b");
    expect(baseName("C:\\")).toBe("C:");
    expect(baseName("/")).toBe("/");
  });

  it("crumbs walk from the root, each with its own full path", () => {
    expect(crumbs("C:\\Users\\문서")).toEqual([
      { label: "C:", path: "C:\\" },
      { label: "Users", path: "C:\\Users" },
      { label: "문서", path: "C:\\Users\\문서" },
    ]);
    expect(crumbs("/home/me")).toEqual([
      { label: "/", path: "/" },
      { label: "home", path: "/home" },
      { label: "me", path: "/home/me" },
    ]);
    expect(crumbs("\\\\wsl.localhost\\Ubuntu\\home")).toEqual([
      { label: "\\\\wsl.localhost\\Ubuntu", path: "\\\\wsl.localhost\\Ubuntu\\" },
      { label: "home", path: "\\\\wsl.localhost\\Ubuntu\\home" },
    ]);
  });
});

describe("typeAhead", () => {
  const list = ["apple", "Banana", "berry", "가방", "나무"];
  it("finds the next name starting with the query, case-insensitively", () => {
    expect(typeAhead(list, "b", 0)).toBe(1);
    expect(typeAhead(list, "b", 2)).toBe(2);
    expect(typeAhead(list, "be", 0)).toBe(2);
  });
  it("wraps around", () => {
    expect(typeAhead(list, "a", 3)).toBe(0);
  });
  it("matches Hangul across NFC/NFD spellings", () => {
    expect(typeAhead(["\u1100\u1161\u1107\u1161\u11bc"], "가", 0)).toBe(0);
    expect(typeAhead(list, "나", 0)).toBe(4);
  });
  it("is -1 when nothing matches", () => {
    expect(typeAhead(list, "zz", 0)).toBe(-1);
  });
});

describe("stemEnd", () => {
  it("selects the name without its extension for a rename", () => {
    expect(stemEnd("report.final.pdf", false)).toBe(12);
    expect(stemEnd(".gitignore", false)).toBe(10);
    expect(stemEnd("Makefile", false)).toBe(8);
    expect(stemEnd("my.dir", true)).toBe(6);
  });
});

describe("formatSize", () => {
  it("formats at the byte/KB/MB/GB boundaries", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(1023)).toBe("1023 B");
    expect(formatSize(1024)).toBe("1.0 KB");
    expect(formatSize(1024 * 1024 - 1)).toBe("1024.0 KB");
    expect(formatSize(1024 * 1024)).toBe("1.0 MB");
    expect(formatSize(5 * 1024 ** 3)).toBe("5.0 GB");
  });
});
