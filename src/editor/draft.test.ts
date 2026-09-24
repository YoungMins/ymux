import { describe, expect, it } from "vitest";
import { draftOffer, encodeDraft, parseDraft, type Draft } from "./draft";

const draft: Draft = {
  v: 1,
  path: "C:\\w\\main.rs",
  text: "fn main() {\n    // 작업 중\n}\n",
  eol: "crlf",
  bom: true,
  base: { modified_ms: 1700, sha256: "abc" },
  savedAt: 42,
};

describe("draft encode/parse", () => {
  it("round-trips", () => {
    expect(parseDraft(encodeDraft(draft))).toEqual(draft);
  });

  it("rejects empty, corrupt and foreign content instead of throwing", () => {
    expect(parseDraft("")).toBeNull();
    expect(parseDraft("{not json")).toBeNull();
    expect(parseDraft("null")).toBeNull();
    expect(parseDraft(JSON.stringify({ ...draft, v: 2 }))).toBeNull();
    expect(parseDraft(JSON.stringify({ ...draft, eol: "weird" }))).toBeNull();
    expect(parseDraft(JSON.stringify({ ...draft, base: null }))).toBeNull();
    expect(parseDraft(JSON.stringify({ ...draft, text: 3 }))).toBeNull();
  });
});

describe("draftOffer", () => {
  it("offers a draft of this file whose text differs from disk", () => {
    expect(draftOffer(draft, "C:\\w\\main.rs", "fn main() {}\n")).toBe("offer");
  });

  it("drops a draft identical to disk, of another file, or missing", () => {
    expect(draftOffer(draft, "C:\\w\\main.rs", draft.text)).toBe("drop");
    expect(draftOffer(draft, "C:\\w\\other.rs", "x")).toBe("drop");
    expect(draftOffer(null, "C:\\w\\main.rs", "x")).toBe("drop");
  });

  it("matches the path across NFC/NFD", () => {
    const d = { ...draft, path: "/w/한글.md".normalize("NFD") };
    expect(draftOffer(d, "/w/한글.md", "other")).toBe("offer");
  });
});
