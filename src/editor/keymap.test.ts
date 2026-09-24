import { describe, expect, it } from "vitest";
import { redo, undo, indentMore } from "@codemirror/commands";
import { gotoLine, findNext, openSearchPanel } from "@codemirror/search";
import { foldCode } from "@codemirror/language";
import type { Command } from "@codemirror/view";
import {
  boundChords,
  effectiveKey,
  filterKeymap,
  isGlobalCmKey,
  parseCmKey,
  type BindingLike,
} from "./keymap";
import { editorKeymap } from "./editorKeymap";

const save: Command = () => true;

describe("parseCmKey", () => {
  it("maps Mod to Cmd on macOS and Ctrl elsewhere", () => {
    expect(parseCmKey("Mod-s", true)).toMatchObject({ metaKey: true, ctrlKey: false, code: "KeyS" });
    expect(parseCmKey("Mod-s", false)).toMatchObject({ metaKey: false, ctrlKey: true, code: "KeyS" });
  });

  it("reads every modifier spelling CM6 accepts and keeps a trailing '-' as the key", () => {
    expect(parseCmKey("Shift-Alt-ArrowUp", false)).toMatchObject({
      shiftKey: true,
      altKey: true,
      key: "ArrowUp",
    });
    expect(parseCmKey("Ctrl-Shift-[", false)).toMatchObject({
      ctrlKey: true,
      shiftKey: true,
      code: "BracketLeft",
    });
    expect(parseCmKey("Mod--", false)).toMatchObject({ ctrlKey: true, key: "-", code: "Minus" });
    expect(parseCmKey("Cmd-Alt-[", true)).toMatchObject({ metaKey: true, altKey: true });
  });
});

describe("effectiveKey", () => {
  it("follows CM6's platform overrides", () => {
    const b: BindingLike = { key: "Mod-y", mac: "Mod-Shift-z" };
    expect(effectiveKey(b, true)).toBe("Mod-Shift-z");
    expect(effectiveKey(b, false)).toBe("Mod-y");
    expect(effectiveKey({ mac: "Ctrl-a" }, false)).toBeNull();
    expect(effectiveKey({ linux: "Ctrl-Shift-z" }, false)).toBeNull();
  });
});

describe("filterKeymap", () => {
  it("drops a binding on a ymux chord and keeps everything else", () => {
    const out = filterKeymap(
      [
        { key: "Mod-Shift-k", run: "deleteLine" },
        { key: "Mod-Alt-g", run: "gotoLine" },
        { key: "Mod-f", run: "search" },
        { key: "Mod-d", run: "selectNext" },
        { key: "Alt-ArrowUp", run: "moveLineUp" },
      ],
      false,
    );
    expect(out.map((b) => b.run)).toEqual(["selectNext", "moveLineUp"]);
  });

  it("keeps the unshifted chord but strips a Shift variant that is global", () => {
    const [b] = filterKeymap([{ key: "Mod-g", run: "next", shift: "prev" }], false);
    expect(b.key).toBe("Mod-g");
    expect(b.shift).toBeUndefined();
    const [f3] = filterKeymap([{ key: "F3", run: "next", shift: "prev" }], false);
    expect(f3.shift).toBe("prev");
  });

  it("resolves platform overrides into a single key", () => {
    const [mac] = filterKeymap([{ key: "Alt-l", mac: "Ctrl-l", run: "selectLine" }], true);
    expect(mac).toEqual({ key: "Ctrl-l", run: "selectLine" });
  });
});

/// What the editor actually binds on each platform: the pre-check table.
for (const isMac of [false, true]) {
  const label = isMac ? "macOS" : "Windows";
  describe(`editorKeymap on ${label}`, () => {
    const km = editorKeymap(isMac, { save });
    const runOf = (chord: string) => km.find((b) => b.key === chord)?.run;

    it("binds no chord ymux's global handler owns (Shift variants included)", () => {
      const leaks = boundChords(km).filter((c) => isGlobalCmKey(c, isMac));
      expect(leaks).toEqual([]);
    });

    it("saves on Mod-s and goes to a line on Mod-g", () => {
      expect(runOf("Mod-s")).toBe(save);
      expect(runOf("Mod-g")).toBe(gotoLine);
    });

    it("leaves Mod-f to main.ts (which opens the search panel through the pane)", () => {
      expect(km.some((b) => b.run === openSearchPanel)).toBe(false);
    });

    it("undoes on Mod-z and redoes on Mod-y — never on Mod-Shift-z (zoom)", () => {
      expect(runOf("Mod-z")).toBe(undo);
      expect(runOf("Mod-y")).toBe(redo);
      expect(km.some((b) => b.key === "Mod-Shift-z" || b.key === "Ctrl-Shift-z")).toBe(false);
    });

    it("keeps find-next on F3 with find-previous on Shift+F3", () => {
      const f3 = km.find((b) => b.key === "F3");
      expect(f3?.run).toBe(findNext);
      expect(f3?.shift).toBeDefined();
    });

    it("has no fold chord (fold is the gutter; Ctrl+Shift+[ is prev-tab)", () => {
      expect(km.some((b) => b.run === foldCode)).toBe(false);
    });

    it("keeps mod+Shift selection on Home/End, drops it on ←/→ (swap pane)", () => {
      const chords = boundChords(km);
      expect(chords).toContain("Shift-Mod-Home");
      expect(chords).toContain("Shift-Mod-End");
      expect(chords).not.toContain("Shift-Mod-ArrowLeft");
      expect(chords).not.toContain("Shift-Mod-ArrowRight");
    });

    it("indents on Tab", () => {
      expect(runOf("Tab")).toBe(indentMore);
    });
  });
}
