import { describe, it, expect } from "vitest";
import {
  ImeBridge,
  commonPrefixLength,
  isCompositionKey,
  mirrorEdit,
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

interface Harness {
  bridge: ImeBridge;
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
  const bridge = new ImeBridge({
    root: target,
    textarea,
    view,
    font: () => ({ family: "MesloLGS NF", size: 14 }),
    send: (data) => sent.push(data),
  });
  bridge.install();
  return {
    bridge,
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

  it("tracks whether a composition is open", () => {
    const h = harness();
    expect(h.bridge.isComposing).toBe(false);
    h.target.dispatch("compositionstart");
    expect(h.bridge.isComposing).toBe(true);
    h.target.dispatch("compositionend", { data: "가" });
    expect(h.bridge.isComposing).toBe(false);
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
