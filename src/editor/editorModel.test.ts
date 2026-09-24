import { describe, expect, it } from "vitest";
import { Text } from "@codemirror/state";
import {
  closeState,
  draftFileAction,
  conflictDecision,
  copyCandidate,
  cursorAfterReload,
  fileName,
  isDocDirty,
  languageForPath,
  openFileDecision,
  pollStep,
  writeArgsFor,
} from "./editorModel";

describe("languageForPath", () => {
  it("maps the v1 grammars by extension", () => {
    expect(languageForPath("C:\\src\\main.rs")).toBe("rust");
    expect(languageForPath("/w/App.tsx")).toBe("tsx");
    expect(languageForPath("/w/types.d.ts")).toBe("typescript");
    expect(languageForPath("/w/x.mjs")).toBe("javascript");
    expect(languageForPath("/w/c.json")).toBe("json");
    expect(languageForPath("/w/App.svelte")).toBe("html");
    expect(languageForPath("/w/README.md")).toBe("markdown");
    expect(languageForPath("/w/ci.yml")).toBe("yaml");
    expect(languageForPath("/w/a.py")).toBe("python");
    expect(languageForPath("/w/s.css")).toBe("css");
  });

  it("is case-insensitive", () => {
    expect(languageForPath("D:\\LIB.RS")).toBe("rust");
  });

  it("gives plain text to no extension, a bare dotfile and an unknown extension", () => {
    expect(languageForPath("/w/Makefile")).toBeNull();
    expect(languageForPath("/home/me/.bashrc")).toBeNull();
    expect(languageForPath("/w/x.unknownext")).toBeNull();
    expect(languageForPath("")).toBeNull();
  });

  it("reads a dotfile's real extension", () => {
    expect(languageForPath("/w/.eslintrc.json")).toBe("json");
  });

  it("does not take a directory's dot for the file's", () => {
    expect(languageForPath("/w/v1.2/Makefile")).toBeNull();
    expect(fileName("C:\\a.b\\c")).toBe("c");
  });
});

describe("isDocDirty", () => {
  const saved = Text.of(["fn main() {", "}"]);

  it("is clean for the saved document itself and for equal content", () => {
    expect(isDocDirty(saved, saved)).toBe(false);
    expect(isDocDirty(saved, Text.of(["fn main() {", "}"]))).toBe(false);
  });

  it("is dirty for a same-length change (the length shortcut cannot decide it)", () => {
    expect(isDocDirty(saved, Text.of(["fn maim() {", "}"]))).toBe(true);
  });

  it("is dirty for a length change", () => {
    expect(isDocDirty(saved, Text.of(["fn main() {", "}", ""]))).toBe(true);
  });
});

describe("pollStep", () => {
  it("skips the read when the mtime matches", () => {
    expect(pollStep({ modified_ms: 5 }, 5)).toBe("unchanged");
    expect(pollStep({ modified_ms: 6 }, 5)).toBe("read");
    expect(pollStep(null, 5)).toBe("missing");
  });
});

describe("conflictDecision — all four stamp × dirty combinations, plus deletion", () => {
  it("unchanged on disk: nothing, clean or dirty", () => {
    expect(conflictDecision("aa", "aa", false)).toBe("none");
    expect(conflictDecision("aa", "aa", true)).toBe("none");
  });

  it("changed on disk: a clean buffer reloads, a dirty one asks", () => {
    expect(conflictDecision("bb", "aa", false)).toBe("reload");
    expect(conflictDecision("bb", "aa", true)).toBe("prompt");
  });

  it("deleted on disk is its own case, whatever the buffer", () => {
    expect(conflictDecision(null, "aa", false)).toBe("deleted");
    expect(conflictDecision(null, "aa", true)).toBe("deleted");
  });
});

describe("openFileDecision (viewer-tab reuse)", () => {
  it("the same file only takes focus, even with edits", () => {
    expect(openFileDecision("/w/a.rs", true, "/w/a.rs")).toBe("focus");
    expect(openFileDecision("/w/a.rs", false, "/w/a.rs")).toBe("focus");
  });

  it("treats NFC and NFD spellings of one Hangul name as the same file", () => {
    const nfd = "/w/한글.md".normalize("NFD");
    expect(openFileDecision(nfd, true, "/w/한글.md")).toBe("focus");
  });

  it("another file replaces a clean buffer and asks over a dirty one", () => {
    expect(openFileDecision("/w/a.rs", false, "/w/b.rs")).toBe("open");
    expect(openFileDecision("/w/a.rs", true, "/w/b.rs")).toBe("ask");
    expect(openFileDecision(null, false, "/w/b.rs")).toBe("open");
  });

  it("the same file in a pane whose load failed is a retry, even with a draft pending", () => {
    expect(openFileDecision("/w/a.rs", true, "/w/a.rs", false)).toBe("retry");
    expect(openFileDecision("/w/a.rs", false, "/w/a.rs", false)).toBe("retry");
  });

  it("another file over a failed pane holding a draft asks first", () => {
    // isDirty() is true for a pending draft even with nothing loaded.
    expect(openFileDecision("/w/a.rs", true, "/w/b.rs", false)).toBe("ask");
  });
});

describe("closeState", () => {
  const base = {
    loaded: true,
    readOnly: false,
    dirty: false,
    deletedOnDisk: false,
    pendingDraft: false,
    hasPath: true,
  };

  it("a clean loaded file has nothing to lose", () => {
    expect(closeState(base)).toEqual({ unsaved: false, savable: false });
  });

  it("a dirty file is unsaved and savable", () => {
    expect(closeState({ ...base, dirty: true })).toEqual({ unsaved: true, savable: true });
  });

  it("a file deleted on disk is unsaved even with no edits", () => {
    expect(closeState({ ...base, deletedOnDisk: true }).unsaved).toBe(true);
  });

  it("a pending recovered draft is unsaved but not savable, even over a clean buffer", () => {
    expect(closeState({ ...base, pendingDraft: true })).toEqual({ unsaved: true, savable: false });
    expect(closeState({ ...base, pendingDraft: true, dirty: true }).savable).toBe(false);
  });

  it("a pending draft in a pane whose file could not be read still blocks the close", () => {
    expect(closeState({ ...base, loaded: false, pendingDraft: true })).toEqual({
      unsaved: true,
      savable: false,
    });
    expect(closeState({ ...base, readOnly: true, pendingDraft: true }).unsaved).toBe(true);
  });

  it("read-only and not-loaded panes never block a close", () => {
    expect(closeState({ ...base, readOnly: true, dirty: true }).unsaved).toBe(false);
    expect(closeState({ ...base, loaded: false, dirty: true }).unsaved).toBe(false);
  });
});

describe("draftFileAction", () => {
  it("never touches a draft that has not been checked or answered yet", () => {
    expect(draftFileAction(true, true)).toBe("keep");
    expect(draftFileAction(true, false)).toBe("keep");
  });

  it("drafts a dirty buffer and drops the draft of a clean one", () => {
    expect(draftFileAction(false, true)).toBe("write");
    expect(draftFileAction(false, false)).toBe("delete");
  });

  it("drafts a clean buffer whose file was deleted — it is the only copy", () => {
    expect(draftFileAction(false, false, true)).toBe("write");
    expect(draftFileAction(false, true, true)).toBe("write");
    expect(draftFileAction(true, false, true)).toBe("keep");
  });
});

describe("writeArgsFor", () => {
  const stamp = { modified_ms: 7, sha256: "abc" };
  const base = { path: "C:\\w\\a.rs", text: "x\n", eol: "crlf" as const, bom: true, stamp, goneOnDisk: false };

  it("guards the write with the loaded stamp", () => {
    expect(writeArgsFor(base).expect).toEqual(stamp);
  });

  it("passes the BOM and the line ending back as read", () => {
    const a = writeArgsFor(base);
    expect(a.bom).toBe(true);
    expect(a.eol).toBe("crlf");
    expect(writeArgsFor({ ...base, bom: false }).bom).toBe(false);
  });

  it("writes a mixed-ending file as LF", () => {
    expect(writeArgsFor({ ...base, eol: "mixed" }).eol).toBe("lf");
  });

  it("drops the guard only to recreate a file confirmed gone", () => {
    expect(writeArgsFor({ ...base, goneOnDisk: true }).expect).toBeNull();
  });
});

describe("cursorAfterReload", () => {
  const lens = [10, 3, 0];
  const len = (l: number) => lens[l - 1];

  it("keeps the line and column when they still exist", () => {
    expect(cursorAfterReload(1, 4, 3, len)).toEqual({ line: 1, col: 4 });
  });

  it("clamps a column past the end of a now-shorter line", () => {
    expect(cursorAfterReload(2, 9, 3, len)).toEqual({ line: 2, col: 3 });
  });

  it("clamps a line past the end of a now-shorter file", () => {
    expect(cursorAfterReload(40, 2, 3, len)).toEqual({ line: 3, col: 0 });
  });
});

describe("copyCandidate", () => {
  it("names copies beside the original, keeping the extension", () => {
    expect(copyCandidate("C:\\w\\main.rs", 1)).toBe("C:\\w\\main (copy).rs");
    expect(copyCandidate("/w/main.rs", 2)).toBe("/w/main (copy 2).rs");
    expect(copyCandidate("/w/Makefile", 1)).toBe("/w/Makefile (copy)");
    expect(copyCandidate("/w/.env", 1)).toBe("/w/.env (copy)");
  });
});
