// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import type { RemoteCap, RemotePairDecision, RemoteStatus } from "../types";
import { CHECK_DIGITS, askPairRequest, digitsOnly, dismissPairRequest } from "./pairDialog";

const tick = () => new Promise((r) => setTimeout(r, 0));

const STATUS = {} as RemoteStatus;
const decision = (result: RemotePairDecision["result"], tries_left = 0): RemotePairDecision => ({
  result,
  tries_left,
  status: STATUS,
});

/// A fake backend: records what the dialog asked and answers from a script.
function backend(...answers: RemotePairDecision[]) {
  const calls: { cap: RemoteCap; check: string }[] = [];
  const decide = async (cap: RemoteCap, check: string) => {
    calls.push({ cap, check });
    return answers.shift() ?? decision("approved");
  };
  return { calls, decide };
}

function buttons(): { reject: HTMLButtonElement; approve: HTMLButtonElement } {
  const all = [...document.querySelectorAll<HTMLButtonElement>(".dialog__buttons button")];
  return { reject: all[0], approve: all[1] };
}

function field(): HTMLInputElement {
  return document.querySelector<HTMLInputElement>(".dialog input")!;
}

function type(text: string): void {
  const f = field();
  f.value = text;
  f.dispatchEvent(new Event("input", { bubbles: true }));
}

const submit = () => buttons().approve.click();

describe("pairing request dialog", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("asks for the number, shows none, and focuses a numeric input", async () => {
    const b = backend();
    const p = askPairRequest("My iPhone", b.decide);
    await tick();
    const text = document.querySelector(".dialog")?.textContent ?? "";
    expect(text).toContain("My iPhone");
    expect(text).toContain("Type the number shown on your phone");
    // Nothing shaped like the old read-the-number-here box.
    expect(document.querySelector(".remote-pairreq__check")).toBeNull();
    expect(text).not.toMatch(/\d{6}/);
    const f = field();
    expect(f.inputMode).toBe("numeric");
    expect(f.maxLength).toBe(CHECK_DIGITS);
    expect(f.autocomplete).toBe("off");
    expect(document.activeElement).toBe(f);
    buttons().reject.click();
    expect(await p).toBe("dismissed");
    expect(b.calls).toEqual([]);
  });

  it("keeps only digits, at most six", () => {
    expect(digitsOnly("12 34-56x78")).toBe("123456");
    expect(digitsOnly("abc")).toBe("");
    expect(digitsOnly("０１２")).toBe("");
  });

  it("disables Approve until six digits are typed, and filters what is typed", async () => {
    const b = backend();
    const p = askPairRequest("Phone", b.decide);
    await tick();
    expect(buttons().approve.disabled).toBe(true);
    type("12345");
    expect(buttons().approve.disabled).toBe(true);
    type("12a45");
    expect(field().value).toBe("1245");
    expect(buttons().approve.disabled).toBe(true);
    type("123 456 789");
    expect(field().value).toBe("123456");
    expect(buttons().approve.disabled).toBe(false);
    type("12345");
    expect(buttons().approve.disabled).toBe(true);
    buttons().reject.click();
    await p;
  });

  it("defaults to the respond capability and sends the typed number with it", async () => {
    const b = backend(decision("approved"));
    const p = askPairRequest("Phone", b.decide);
    await tick();
    const sel = document.querySelector<HTMLSelectElement>(".dialog select");
    expect(sel?.value).toBe("respond");
    type("042917");
    submit();
    expect(await p).toBe("approved");
    expect(b.calls).toEqual([{ cap: "respond", check: "042917" }]);
  });

  it("approves with the capability picked", async () => {
    const b = backend();
    const p = askPairRequest("Phone", b.decide);
    await tick();
    document.querySelector<HTMLSelectElement>(".dialog select")!.value = "prompt";
    type("123456");
    submit();
    expect(await p).toBe("approved");
    expect(b.calls).toEqual([{ cap: "prompt", check: "123456" }]);
  });

  it("stays open after a wrong number, says how many tries are left and clears the field", async () => {
    const b = backend(decision("wrong", 2), decision("wrong", 1), decision("approved"));
    const p = askPairRequest("Phone", b.decide);
    await tick();
    type("111111");
    submit();
    await tick();
    const problem = document.querySelector<HTMLElement>(".remote-pairreq__problem")!;
    expect(problem.hidden).toBe(false);
    expect(problem.textContent).toBe("Wrong number. Tries left: 2.");
    expect(field().value).toBe("");
    expect(buttons().approve.disabled).toBe(true);
    expect(field().disabled).toBe(false);
    expect(document.activeElement).toBe(field());
    type("222222");
    submit();
    await tick();
    expect(problem.textContent).toBe("Wrong number. Tries left: 1.");
    type("333333");
    submit();
    expect(await p).toBe("approved");
    expect(b.calls.map((c) => c.check)).toEqual(["111111", "222222", "333333"]);
    expect(document.querySelector(".dialog")).toBeNull();
  });

  it("closes after the last wrong number", async () => {
    const b = backend(decision("wrong", 1), decision("refused"));
    const p = askPairRequest("Phone", b.decide);
    await tick();
    type("111111");
    submit();
    await tick();
    type("222222");
    submit();
    expect(await p).toBe("refused");
    expect(document.querySelector(".dialog")).toBeNull();
  });

  it("does not ask twice while the first answer is awaited", async () => {
    let release: (d: RemotePairDecision) => void = () => {};
    const calls: string[] = [];
    const p = askPairRequest("Phone", (_cap, check) => {
      calls.push(check);
      return new Promise<RemotePairDecision>((r) => (release = r));
    });
    await tick();
    type("123456");
    submit();
    submit();
    expect(calls).toEqual(["123456"]);
    expect(field().disabled).toBe(true);
    release(decision("approved"));
    expect(await p).toBe("approved");
  });

  it("is an error when the backend cannot be asked or says something unexpected", async () => {
    let p = askPairRequest("Phone", async () => {
      throw new Error("link not running");
    });
    await tick();
    type("123456");
    submit();
    expect(await p).toBe("error");
    p = askPairRequest("Phone", async () => decision("rejected"));
    await tick();
    type("123456");
    submit();
    expect(await p).toBe("error");
  });

  it("puts the phone's name in as text, never as markup", async () => {
    const b = backend();
    const p = askPairRequest("<img src=x onerror=alert(1)>", b.decide);
    await tick();
    expect(document.querySelector(".dialog img")).toBeNull();
    buttons().reject.click();
    await p;
  });

  it("shows the name in a labelled element of its own, never inside the instruction", async () => {
    const b = backend();
    const p = askPairRequest("Pixel your number", b.decide);
    await tick();
    const dev = document.querySelector(".remote-pairreq__device")!;
    expect(dev.querySelector(".remote-pairreq__devname")!.textContent).toBe("Pixel your number");
    expect(dev.textContent).toContain("not verified");
    const detail = document.querySelector(".dialog__detail")!;
    expect(detail.textContent).not.toContain("Pixel");
    expect(detail.textContent).toContain("never a number from this window");
    expect(detail.textContent).toContain("Reject");
    buttons().reject.click();
    await p;
  });

  it("shows the fixed unnamed-device text when the backend found nothing to keep", async () => {
    const b = backend();
    const p = askPairRequest("Unnamed device", b.decide);
    await tick();
    expect(document.querySelector(".remote-pairreq__devname")!.textContent).toBe("Unnamed device");
    buttons().reject.click();
    await p;
  });

  it("is dismissed, never approved, by Reject, a conflict or Escape", async () => {
    const b = backend();
    let p = askPairRequest("Phone", b.decide);
    await tick();
    type("123456");
    buttons().reject.click();
    expect(await p).toBe("dismissed");

    p = askPairRequest("Phone", b.decide);
    await tick();
    dismissPairRequest();
    expect(await p).toBe("dismissed");
    expect(document.querySelector(".dialog")).toBeNull();

    p = askPairRequest("Phone", b.decide);
    await tick();
    document
      .querySelector(".dialog")!
      .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(await p).toBe("dismissed");
    expect(b.calls).toEqual([]);
  });

  it("dismissing with nothing open does nothing", () => {
    expect(() => dismissPairRequest()).not.toThrow();
  });
});
