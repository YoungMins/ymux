import { describe, it, expect } from "vitest";
import { actionDir, initialNav, listingFailed, listingLanded, navigateTo, isSameDir } from "./navState";

describe("navState — actions act on the folder that is listed", () => {
  it("has no action target before anything is listed", () => {
    expect(actionDir(initialNav("/a"))).toBeNull();
  });

  it("targets the listed folder once its listing lands", () => {
    const s = listingLanded(initialNav("/a"), "/a");
    expect(actionDir(s)).toBe("/a");
  });

  it("refuses while a navigation is pending: the list on screen is the old folder", () => {
    let s = listingLanded(initialNav("/a"), "/a");
    s = navigateTo(s, "/b");
    expect(s.requested).toBe("/b");
    expect(s.listed).toBe("/a");
    expect(actionDir(s)).toBeNull();
  });

  it("ignores a late listing of a folder no longer requested", () => {
    let s = listingLanded(initialNav("/a"), "/a");
    s = navigateTo(s, "/b");
    s = navigateTo(s, "/c");
    s = listingLanded(s, "/b");
    expect(actionDir(s)).toBeNull();
    s = listingLanded(s, "/c");
    expect(actionDir(s)).toBe("/c");
  });

  it("has nothing to act on after a failed listing", () => {
    let s = listingLanded(initialNav("/a"), "/a");
    s = navigateTo(s, "/gone");
    s = listingFailed(s, "/gone");
    expect(s.listed).toBeNull();
    expect(actionDir(s)).toBeNull();
  });

  it("isSameDir compares the pane's own spellings, NFC-only (rule 15)", () => {
    expect(isSameDir("/w/한", "/w/한")).toBe(true);
    expect(isSameDir("/w/a", "/w/A")).toBe(false);
    expect(isSameDir(null, "/w")).toBe(false);
  });
});
