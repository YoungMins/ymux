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
    // The dock's files pane already opened in this dir.
    follow.reset("/start");
    follow.activePaneChanged("a", "/start");
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([]);
  });

  // macOS hands back decomposed filenames. The same Korean directory then
  // arrives composed from the shell's own OSC 7 payload and decomposed from
  // a path the filesystem produced, and the two are different strings.
  const NFC = "한글";
  const NFD = "한글";

  it("does not re-send a directory whose Hangul arrived decomposed", () => {
    expect(NFC).not.toBe(NFD); // fixture guard: the bytes really differ
    follow.activePaneChanged("a", `/Users/u/${NFC}`);
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    follow.cwdChanged("a", `/Users/u/${NFD}`);
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([`/Users/u/${NFC}`]);
  });

  it("sends the dir as the pane spelled it, not the normalized key", () => {
    // The files pane has to open what it is given, so the decomposed spelling must
    // survive when it is the first thing seen.
    follow.activePaneChanged("a", `/Users/u/${NFD}`);
    vi.advanceTimersByTime(FOLLOW_DEBOUNCE_MS);
    expect(sent).toEqual([`/Users/u/${NFD}`]);
  });
});
