import { describe, it, expect } from "vitest";
import { t, translationGaps } from "./i18n";

describe("i18n completeness (rule 7)", () => {
  it("has every files pane string in all 13 languages", () => {
    expect(t("files.title")).toBe("Files");
    expect(translationGaps("files.")).toEqual([]);
  });

  it("has every editor pane string in all 13 languages", () => {
    expect(t("editor.save")).toBe("Save");
    expect(translationGaps("editor.")).toEqual([]);
    expect(translationGaps("shortcut.save")).toEqual([]);
    expect(translationGaps("shortcut.gotoLine")).toEqual([]);
  });

  it("has every \"+\" launcher string in all 13 languages", () => {
    expect(t("launcher.agents")).toBe("Agents");
    expect(translationGaps("launcher.")).toEqual([]);
  });

  it("has every backend error kind in all 13 languages", () => {
    expect(translationGaps("fsError.")).toEqual([]);
  });
});

describe("no hard-coded shortcuts in translations", () => {
  it("leaves shortcut text to shortcutLabel() at the use site, so macOS shows Cmd", async () => {
    const { readFileSync } = await import("node:fs");
    const { fileURLToPath } = await import("node:url");
    const src = readFileSync(fileURLToPath(new URL("./i18n.ts", import.meta.url)), "utf8");
    expect(src.match(/\b(Ctrl|Alt)\+\S+/g) ?? []).toEqual([]);
    expect(translationGaps("browser.zoom")).toEqual([]);
    expect(translationGaps("notes.buttonTitle")).toEqual([]);
  });
});
