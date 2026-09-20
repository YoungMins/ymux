import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { CwdFollow, FOLLOW_DEBOUNCE_MS } from "./cwdFollow";

describe("CwdFollow", () => {
  let sent: string[];
  let follow: CwdFollow;

  beforeEach(() => {
    vi.useFakeTimers();
    sent = [];
    follow = new CwdFollow((dir) => sent.push(dir));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("sends the active pane's cwd once the debounce elapses", () => {
    follow.activePaneChanged("a", "/work");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS - 1);
    expect(sent).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(sent).toEqual(["/work"]);
  });

  it("collapses a burst of changes into the last one", () => {
    follow.activePaneChanged("a", "/one");
    follow.cwdChanged("a", "/two");
    follow.cwdChanged("a", "/three");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual(["/three"]);
  });

  it("does not resend the directory it sent last", () => {
    follow.activePaneChanged("a", "/work");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    follow.cwdChanged("a", "/work");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual(["/work"]);
  });

  it("ignores cwd changes from panes that are not active", () => {
    follow.activePaneChanged("a", null);
    follow.cwdChanged("b", "/elsewhere");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([]);
  });

  it("follows the newly active pane after a switch", () => {
    follow.activePaneChanged("a", "/a");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    follow.activePaneChanged("b", "/b");
    follow.cwdChanged("a", "/a2");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual(["/a", "/b"]);
  });

  it("drops a pending send when the new active pane has no cwd", () => {
    follow.activePaneChanged("a", "/a");
    follow.activePaneChanged("browser", null);
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([]);
  });

  it("reset() treats the given dir as already sent", () => {
    // A freshly (re)started ydir already opened in this dir.
    follow.reset("/start");
    follow.activePaneChanged("a", "/start");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([]);
  });
});
