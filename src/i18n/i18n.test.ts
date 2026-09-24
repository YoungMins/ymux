import { describe, it, expect } from "vitest";
import { t, translationGaps } from "./i18n";

describe("i18n completeness (rule 7)", () => {
  it("has every files pane string in all 13 languages", () => {
    expect(t("files.title")).toBe("Files");
    expect(translationGaps("files.")).toEqual([]);
  });

  it("has every backend error kind in all 13 languages", () => {
    expect(translationGaps("fsError.")).toEqual([]);
  });
});
