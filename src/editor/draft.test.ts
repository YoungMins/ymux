import { describe, expect, it } from "vitest";
import { coldDraftEntry, draftDisposition, encodeDraft, orphanDraftIds, parseDraft, type Draft } from "./draft";

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

describe("orphanDraftIds", () => {
  const a = "0b9c2c3e-1f2a-4b5c-8d9e-0a1b2c3d4e5f";
  const b = "11111111-2222-3333-4444-555555555555";

  it("lists only drafts whose pane is gone from the config", () => {
    expect(orphanDraftIds([a, b], [a])).toEqual([b]);
  });

  it("keeps a live pane's draft, whatever the case of its id", () => {
    expect(orphanDraftIds([a.toUpperCase()], [a])).toEqual([]);
  });

  it("with no panes at all, everything is an orphan; with no drafts, nothing", () => {
    expect(orphanDraftIds([a], [])).toEqual([a]);
    expect(orphanDraftIds([], [a])).toEqual([]);
  });
});

describe("coldDraftEntry", () => {
  it("lists a never-mounted pane's draft by file name, as unsaved and not savable", () => {
    expect(coldDraftEntry(encodeDraft(draft))).toEqual({ name: "main.rs", dirty: true, hasPath: false });
  });

  it("is null for no draft or a corrupt one", () => {
    expect(coldDraftEntry("")).toBeNull();
    expect(coldDraftEntry("{")).toBeNull();
  });
});

describe("draftDisposition", () => {
  const disk = (text: string) => ({ text, editable: true });

  it("offers Restore for a draft of this file whose text differs from disk", () => {
    expect(draftDisposition(draft, "C:\\w\\main.rs", disk("fn main() {}\n"))).toBe("restore");
  });

  it("drops a draft identical to disk; reports none when there is none", () => {
    expect(draftDisposition(draft, "C:\\w\\main.rs", disk(draft.text))).toBe("drop");
    expect(draftDisposition(null, "C:\\w\\main.rs", disk("x"))).toBe("none");
  });

  it("rescues — never drops — a draft whose file could not be read", () => {
    // Deleted, renamed, locked by AV, no longer text: the draft is the only copy.
    expect(draftDisposition(draft, "C:\\w\\main.rs", null)).toBe("rescue");
  });

  it("rescues a draft whose file is now read-only (grew past the cap)", () => {
    expect(draftDisposition(draft, "C:\\w\\main.rs", { text: "x", editable: false })).toBe("rescue");
    // Even when the head happens to match: the buffer cannot take it.
    expect(draftDisposition(draft, "C:\\w\\main.rs", { text: draft.text, editable: false })).toBe(
      "rescue",
    );
  });

  it("matches the path across NFC/NFD", () => {
    const d = { ...draft, path: "/w/한글.md".normalize("NFD") };
    expect(draftDisposition(d, "/w/한글.md", disk("other"))).toBe("restore");
  });
});
