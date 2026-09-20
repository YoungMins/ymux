import { describe, it, expect } from "vitest";
import {
  ImeBridge,
  commonPrefixLength,
  isCompositionKey,
  isEnterKey,
  mirrorEdit,
  type ImeClock,
  type ImeCompositionView,
  type ImeEvent,
  type ImeEventTarget,
  type ImeListener,
  type ImeTextarea,
} from "./ime";

const DEL = "\x7f";

/// Stand-in for the xterm root element. Records listeners per event type so a
/// test can dispatch to them and — the thing the fix actually depends on —
/// models `stopImmediatePropagation()` preventing the listeners registered
/// after ours (xterm's) from running.
class FakeTarget implements ImeEventTarget {
  private listeners = new Map<string, ImeListener[]>();
  /// Events that got past our handler to a hypothetical xterm-side listener.
  readonly leaked: string[] = [];

  addEventListener(type: string, listener: ImeListener): void {
    const list = this.listeners.get(type) ?? [];
    list.push(listener);
    this.listeners.set(type, list);
  }

  removeEventListener(type: string, listener: ImeListener): void {
    const list = this.listeners.get(type) ?? [];
    const i = list.indexOf(listener);
    if (i >= 0) list.splice(i, 1);
  }

  dispatch(type: string, init: { data?: string; inputType?: string } = {}): void {
    let stopped = false;
    const ev: ImeEvent = {
      ...init,
      stopImmediatePropagation: () => {
        stopped = true;
      },
    };
    for (const listener of [...(this.listeners.get(type) ?? [])]) {
      listener(ev);
      if (stopped) return;
    }
    this.leaked.push(type);
  }
}

/// Timers under the test's control, so the deferred-newline fallback can be
/// exercised without waiting 200 ms. Honours `clearTimeout`, because the
/// bridge cancelling a timer it has already answered is part of what is being
/// tested.
class FakeClock implements ImeClock {
  private pending = new Map<number, () => void>();
  private next = 1;

  setTimeout(handler: () => void): number {
    const id = this.next++;
    this.pending.set(id, handler);
    return id;
  }

  clearTimeout(id: number): void {
    this.pending.delete(id);
  }

  /// Fire every timer still outstanding, as the clock reaching the deadline
  /// would.
  tick(): void {
    const due = [...this.pending.values()];
    this.pending.clear();
    for (const handler of due) handler();
  }

  get outstanding(): number {
    return this.pending.size;
  }
}

interface Harness {
  bridge: ImeBridge;
  clock: FakeClock;
  target: FakeTarget;
  textarea: ImeTextarea;
  view: ImeCompositionView & { active: boolean };
  sent: string[];
  /// One IME keystroke as WKWebView reports it: the textarea is already
  /// updated when `input` fires, and the `keydown` (keyCode 229) trails it.
  key(value: string, inputType: string): void;
}

function harness(): Harness {
  const target = new FakeTarget();
  const textarea: ImeTextarea = {
    value: "",
    style: { left: "48px", top: "96px", height: "17px" },
  };
  const view = {
    textContent: null as string | null,
    style: {
      left: "",
      top: "",
      height: "",
      lineHeight: "",
      fontFamily: "",
      fontSize: "",
    },
    active: false,
    classList: {
      toggle(token: string, force: boolean) {
        if (token === "active") view.active = force;
      },
    },
  };
  const sent: string[] = [];
  const clock = new FakeClock();
  const bridge = new ImeBridge({
    root: target,
    textarea,
    view,
    font: () => ({ family: "MesloLGS NF", size: 14 }),
    send: (data) => sent.push(data),
    clock,
  });
  bridge.install();
  return {
    bridge,
    clock,
    target,
    textarea,
    view,
    sent,
    key(value, inputType) {
      textarea.value = value;
      target.dispatch("input", { inputType, data: value });
      bridge.handleKeyDown({ keyCode: 229, isComposing: false });
    },
  };
}

describe("commonPrefixLength", () => {
  it("counts the shared head", () => {
    expect(commonPrefixLength("안녕하", "안녕핫")).toBe(2);
  });

  it("is zero for a first-character change", () => {
    expect(commonPrefixLength("ㅇ", "아")).toBe(0);
  });

  it("handles either side being empty", () => {
    expect(commonPrefixLength("", "안")).toBe(0);
    expect(commonPrefixLength("안", "")).toBe(0);
  });

  it("counts a full match", () => {
    expect(commonPrefixLength("안녕", "안녕")).toBe(2);
  });
});

describe("mirrorEdit", () => {
  it("sends nothing when the buffer is unchanged", () => {
    expect(mirrorEdit("안녕", "안녕")).toBe("");
  });

  it("appends without retracting when text only grows", () => {
    expect(mirrorEdit("안", "안ㄴ")).toBe("ㄴ");
  });

  it("retracts exactly the characters the IME replaced", () => {
    // `아` → `안`: one character revised in place, so one DEL.
    expect(mirrorEdit("아", "안")).toBe(`${DEL}안`);
  });

  it("retracts a whole tail at once", () => {
    expect(mirrorEdit("안녕하", "안")).toBe(`${DEL}${DEL}`);
  });

  it("retracts and replaces a multi-character tail", () => {
    // The shape a phrase-level IME (Japanese) produces.
    expect(mirrorEdit("にほんご", "に本語")).toBe(`${DEL}${DEL}${DEL}本語`);
  });
});

describe("isCompositionKey", () => {
  it("claims a key the IME consumed", () => {
    // WKWebView's Hangul keydown: `isComposing` is false throughout, because
    // no composition ever starts — only the 229 marker identifies it.
    expect(isCompositionKey({ isComposing: false, keyCode: 229, key: "ㅇ" })).toBe(true);
  });

  it("claims keystrokes inside a real composition", () => {
    expect(isCompositionKey({ isComposing: true, keyCode: 65, key: "a" })).toBe(true);
  });

  it("claims WebKit's `Process` key with no 229 marker", () => {
    expect(isCompositionKey({ isComposing: false, keyCode: 0, key: "Process" })).toBe(true);
  });

  it("leaves ordinary typing alone", () => {
    expect(isCompositionKey({ isComposing: false, keyCode: 65, key: "a" })).toBe(false);
  });

  it("leaves Enter alone, so it still reaches the shell", () => {
    expect(isCompositionKey({ isComposing: false, keyCode: 13, key: "Enter" })).toBe(false);
  });
});

describe("ImeBridge on the WKWebView path (no composition events)", () => {
  /// The exact event sequence captured from the running app, which used to
  /// reach the shell as `ㅇㄴㅎ세요`.
  function typeAnnyeong(h: Harness): void {
    h.key("ㅇ", "insertText");
    h.key("아", "insertReplacementText");
    h.key("안", "insertReplacementText");
    h.key("안ㄴ", "insertText");
    h.key("안녀", "insertReplacementText");
    h.key("안녕", "insertReplacementText");
    h.key("안녕ㅎ", "insertText");
    h.key("안녕하", "insertReplacementText");
    h.key("안녕핫", "insertReplacementText");
    // One keystroke, two events: the ㅅ leaves `하` and opens `세`.
    h.textarea.value = "안녕하";
    h.target.dispatch("input", { inputType: "insertReplacementText", data: "하" });
    h.key("안녕하세", "insertText");
    h.key("안녕하셍", "insertReplacementText");
    h.textarea.value = "안녕하세";
    h.target.dispatch("input", { inputType: "insertReplacementText", data: "세" });
    h.key("안녕하세요", "insertText");
  }

  /// Replay what the terminal would end up holding.
  function applied(sent: string[]): string {
    let out = "";
    for (const ch of sent.join("")) {
      if (ch === DEL) out = out.slice(0, -1);
      else out += ch;
    }
    return out;
  }

  it("delivers the whole phrase, not one jamo per syllable", () => {
    const h = harness();
    typeAnnyeong(h);
    expect(applied(h.sent)).toBe("안녕하세요");
  });

  it("forwards a replacement as a retraction plus the new syllable", () => {
    const h = harness();
    h.key("ㅇ", "insertText");
    h.key("아", "insertReplacementText");
    expect(h.sent).toEqual(["ㅇ", `${DEL}아`]);
  });

  it("does not retract anything when the syllable only grows", () => {
    const h = harness();
    h.key("안", "insertReplacementText");
    h.key("안ㄴ", "insertText");
    expect(h.sent).toEqual(["안", "ㄴ"]);
  });

  it("never lets xterm's own input handler see the event", () => {
    // xterm forwards `insertText` only, which is what shipped a bare jamo per
    // syllable. It must not also act on what we already sent.
    const h = harness();
    h.key("ㅇ", "insertText");
    expect(h.target.leaked).toEqual([]);
  });

  it("keeps composing across a modifier press", () => {
    // Shift, for `ㄲ`. Treating it as the end of the run would strand the
    // mirror and re-send the whole buffer on the next keystroke.
    const h = harness();
    h.key("ㄱ", "insertText");
    expect(h.bridge.handleKeyDown({ keyCode: 16, key: "Shift" })).toBe(false);
    h.key("까", "insertReplacementText");
    expect(applied(h.sent)).toBe("까");
  });

  it("ends the run on a key the terminal owns, and starts the next one clean", () => {
    const h = harness();
    h.key("가", "insertReplacementText");
    // Enter: xterm sends the CR itself, and the shell's line buffer is now
    // empty — so the mirror must not try to retract `가` afterwards.
    expect(h.bridge.handleKeyDown({ keyCode: 13, key: "Enter" })).toBe(false);
    expect(h.textarea.value).toBe("");
    h.key("나", "insertReplacementText");
    expect(h.sent).toEqual(["가", "나"]);
  });

  it("sends a keypress-delivered character once", () => {
    // xterm leaves A–Z to its keypress handler (a caps-lock workaround), and
    // the same keystroke then lands in the textarea as an `input`. Only one of
    // the two may reach the PTY.
    const h = harness();
    expect(h.bridge.handleKeyDown({ keyCode: 65, key: "A" })).toBe(false);
    h.target.dispatch("keypress");
    h.textarea.value = "A";
    h.target.dispatch("input", { inputType: "insertText", data: "A" });
    expect(h.sent).toEqual(["A"]);
    expect(h.target.leaked).toEqual([]);
  });

  it("reports IME keys as xterm's to ignore and other keys as xterm's to handle", () => {
    const h = harness();
    expect(h.bridge.handleKeyDown({ keyCode: 229, key: "ㅇ" })).toBe(true);
    expect(h.bridge.handleKeyDown({ keyCode: 65, key: "a" })).toBe(false);
  });
});

describe("ImeBridge on the composition-event path", () => {
  it("commits exactly what the IME says it committed", () => {
    const h = harness();
    h.target.dispatch("compositionstart");
    h.textarea.value = "ㅎ";
    h.target.dispatch("compositionupdate", { data: "ㅎ" });
    h.textarea.value = "하";
    h.target.dispatch("compositionupdate", { data: "하" });
    h.target.dispatch("compositionend", { data: "하" });
    expect(h.sent).toEqual(["하"]);
  });

  it("holds the mirror back while a composition is open", () => {
    // Otherwise the half-built syllable goes out twice: once from the input
    // mirror, once from `compositionend`.
    const h = harness();
    h.target.dispatch("compositionstart");
    h.textarea.value = "하";
    h.target.dispatch("input", { inputType: "insertCompositionText", data: "하" });
    expect(h.sent).toEqual([]);
    h.target.dispatch("compositionend", { data: "하" });
    expect(h.sent).toEqual(["하"]);
  });

  it("leaves no residue for the next run to retract", () => {
    const h = harness();
    h.target.dispatch("compositionstart");
    h.textarea.value = "하";
    h.target.dispatch("compositionend", { data: "하" });
    h.key("가", "insertText");
    expect(h.sent).toEqual(["하", "가"]);
  });

  it("sends nothing when a composition is cancelled", () => {
    const h = harness();
    h.target.dispatch("compositionstart");
    h.target.dispatch("compositionupdate", { data: "ㅎ" });
    h.target.dispatch("compositionend", { data: "" });
    expect(h.sent).toEqual([]);
    expect(h.bridge.isComposing).toBe(false);
  });

  it("shows the in-progress text on the caret and hides it on commit", () => {
    const h = harness();
    h.target.dispatch("compositionstart");
    h.target.dispatch("compositionupdate", { data: "ㅎ" });
    expect(h.view.textContent).toBe("ㅎ");
    expect(h.view.active).toBe(true);
    // Borrowed from the helper textarea, which xterm parks on the cursor cell.
    expect(h.view.style.left).toBe("48px");
    expect(h.view.style.top).toBe("96px");
    expect(h.view.style.lineHeight).toBe("17px");
    expect(h.view.style.fontFamily).toBe("MesloLGS NF");
    expect(h.view.style.fontSize).toBe("14px");

    h.target.dispatch("compositionend", { data: "하" });
    expect(h.view.active).toBe(false);
  });

  it("sends a space that commits a syllable exactly once", () => {
    // WebView2 + the Windows Korean IME, pressing Space on `하`: the keydown
    // is the IME's (229), the syllable commits, and the space then arrives as
    // a `keypress` *and* an `input`. xterm's `_keyPress` would send it (its
    // keydown was skipped, so `_keyDownHandled` is false) and the mirror would
    // send it again — `하  ` on the PTY.
    const h = harness();
    h.target.dispatch("compositionstart");
    h.textarea.value = "하";
    h.target.dispatch("compositionupdate", { data: "하" });
    expect(h.bridge.handleKeyDown({ keyCode: 229, key: "Process" })).toBe(true);
    h.target.dispatch("compositionend", { data: "하" });
    h.target.dispatch("keypress");
    h.textarea.value = " ";
    h.target.dispatch("input", { inputType: "insertText", data: " " });
    expect(h.sent).toEqual(["하", " "]);
    expect(h.target.leaked).toEqual([]);
  });

  it("tracks whether a composition is open", () => {
    const h = harness();
    expect(h.bridge.isComposing).toBe(false);
    h.target.dispatch("compositionstart");
    expect(h.bridge.isComposing).toBe(true);
    h.target.dispatch("compositionend", { data: "가" });
    expect(h.bridge.isComposing).toBe(false);
  });
});

describe("isEnterKey", () => {
  it("claims Enter even when the IME has rewritten every other field", () => {
    // WebView2 committing a Hangul syllable with Return. `keyCode === 13` is
    // gone; `code` is the only thing left that says Enter.
    expect(isEnterKey({ keyCode: 229, key: "Process", code: "Enter" })).toBe(true);
  });

  it("claims the keypad's Enter", () => {
    expect(isEnterKey({ keyCode: 13, key: "Enter", code: "NumpadEnter" })).toBe(true);
  });

  it("trusts `code` over `keyCode` when both are present", () => {
    expect(isEnterKey({ keyCode: 13, key: "Enter", code: "KeyA" })).toBe(false);
  });

  it("falls back to keyCode when the event carries no `code`", () => {
    expect(isEnterKey({ keyCode: 13, key: "Enter" })).toBe(true);
    expect(isEnterKey({ keyCode: 65, key: "a" })).toBe(false);
  });
});

describe("ImeBridge deferred newline", () => {
  /// A composition open with `하` in it, as WebView2 reports it.
  function composing(h: Harness): void {
    h.target.dispatch("compositionstart");
    h.textarea.value = "하";
    h.target.dispatch("compositionupdate", { data: "하" });
  }

  it("holds Enter back until the composition commits", () => {
    const h = harness();
    composing(h);
    // Claimed by the bridge: xterm must not send the CR yet.
    expect(h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" })).toBe(true);
    expect(h.sent).toEqual([]);
    h.target.dispatch("compositionend", { data: "하" });
    // Text first, CR second — the ordering is the entire feature. Reversed,
    // the shell runs an empty line and leaves the syllable on the next prompt.
    expect(h.sent).toEqual(["하", "\r"]);
  });

  it("sends the Enter anyway if the composition never commits", () => {
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    expect(h.sent).toEqual([]);
    h.clock.tick();
    expect(h.sent).toEqual(["\r"]);
  });

  it("sends exactly one CR when the commit beats the fallback timer", () => {
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.target.dispatch("compositionend", { data: "하" });
    h.clock.tick();
    expect(h.sent).toEqual(["하", "\r"]);
    expect(h.clock.outstanding).toBe(0);
  });

  it("collapses a repeated Enter inside one composition into one CR", () => {
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.target.dispatch("compositionend", { data: "하" });
    expect(h.sent).toEqual(["하", "\r"]);
  });

  it("does not let a later composition swallow an Enter held by an earlier one", () => {
    // The stale-release hazard: the Enter belongs to the first composition, so
    // it goes out when that one is abandoned — not after the second one's text.
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.target.dispatch("compositionstart");
    expect(h.sent).toEqual(["\r"]);
    h.textarea.value = "가";
    h.target.dispatch("compositionend", { data: "가" });
    h.clock.tick();
    expect(h.sent).toEqual(["\r", "가"]);
  });

  it("still sends the Enter when the composition is cancelled outright", () => {
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.target.dispatch("compositionend", { data: "" });
    expect(h.sent).toEqual(["\r"]);
  });

  it("passes Enter straight through on the WKWebView path", () => {
    // macOS fires no composition events at all, so there is nothing to wait
    // for: the bridge must decline the key and let xterm send the CR itself,
    // exactly as before.
    const h = harness();
    h.key("가", "insertReplacementText");
    expect(h.bridge.handleKeyDown({ keyCode: 13, key: "Enter", code: "Enter" })).toBe(false);
    expect(h.sent).toEqual(["가"]);
    expect(h.textarea.value).toBe("");
    expect(h.clock.outstanding).toBe(0);
  });

  it("drops a held Enter when the pane goes away", () => {
    const h = harness();
    composing(h);
    h.bridge.handleKeyDown({ keyCode: 229, key: "Process", code: "Enter" });
    h.bridge.dispose();
    h.clock.tick();
    expect(h.sent).toEqual([]);
  });
});

describe("ImeBridge lifecycle", () => {
  it("survives a terminal with no composition view", () => {
    const target = new FakeTarget();
    const textarea: ImeTextarea = {
      value: "",
      style: { left: "", top: "", height: "" },
    };
    const sent: string[] = [];
    const bridge = new ImeBridge({
      root: target,
      textarea,
      view: null,
      font: () => ({ family: "monospace", size: 14 }),
      send: (data) => sent.push(data),
    });
    bridge.install();
    target.dispatch("compositionstart");
    target.dispatch("compositionupdate", { data: "ㅎ" });
    target.dispatch("compositionend", { data: "하" });
    expect(sent).toEqual(["하"]);
  });

  it("stops intercepting once disposed", () => {
    const h = harness();
    h.bridge.dispose();
    h.textarea.value = "하";
    h.target.dispatch("input", { inputType: "insertText", data: "하" });
    expect(h.sent).toEqual([]);
    expect(h.target.leaked).toEqual(["input"]);
  });
});
