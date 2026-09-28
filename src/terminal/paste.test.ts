import { describe, it, expect } from "vitest";
import {
  CHUNK_SIZE,
  DIRECT_LIMIT,
  claimPasteEvent,
  decideImagePaste,
  pasteKeyOwner,
  preparePaste,
  runPaste,
  sanitizePaste,
  type PasteEventLike,
  type PasteRun,
} from "./paste";

const START = "\x1b[200~";
const END = "\x1b[201~";
const ESC_SYMBOL = "␛";

/// What the PTY ends up with, for the cases where only the whole matters.
function joined(chunks: string[]): string {
  return chunks.join("");
}

describe("sanitizePaste", () => {
  it("turns CRLF into a bare CR", () => {
    expect(sanitizePaste("a\r\nb")).toBe("a\rb");
  });

  it("turns a lone LF into a CR", () => {
    expect(sanitizePaste("a\nb\nc")).toBe("a\rb\rc");
  });

  it("leaves an existing bare CR alone", () => {
    expect(sanitizePaste("a\rb")).toBe("a\rb");
  });

  it("replaces every ESC with the escape symbol", () => {
    expect(sanitizePaste("\x1b[31mred\x1b[0m")).toBe(
      `${ESC_SYMBOL}[31mred${ESC_SYMBOL}[0m`,
    );
  });

  it("keeps the payload the same length when defanging ESC", () => {
    const text = "\x1b\x1b\x1b";
    expect(sanitizePaste(text)).toHaveLength(text.length);
  });
});

describe("preparePaste", () => {
  it("writes nothing at all for empty input", () => {
    expect(preparePaste("", { bracketed: true })).toEqual([]);
    expect(preparePaste("", { bracketed: false })).toEqual([]);
  });

  it("sends plain text unwrapped when the app has not enabled DECSET 2004", () => {
    expect(preparePaste("ls -la", { bracketed: false })).toEqual(["ls -la"]);
  });

  it("wraps in the bracketed-paste frame when the app has enabled it", () => {
    expect(preparePaste("ls -la", { bracketed: true })).toEqual([
      `${START}ls -la${END}`,
    ]);
  });

  it("normalizes newlines inside the frame", () => {
    expect(preparePaste("one\r\ntwo\nthree", { bracketed: true })).toEqual([
      `${START}one\rtwo\rthree${END}`,
    ]);
  });

  it("defangs a pasted end marker so it cannot close the frame early", () => {
    // The attack: clipboard text carrying its own `ESC[201~`. Left intact, the
    // shell would treat everything after it as typed keystrokes and run
    // `rm -rf /` without the user pressing Enter.
    const evil = `safe${END}rm -rf /\n`;
    const [only] = preparePaste(evil, { bracketed: true });
    expect(only).toBe(`${START}safe${ESC_SYMBOL}[201~rm -rf /\r${END}`);
    // Exactly one opener and one closer survive: our own.
    expect(only.split(START)).toHaveLength(2);
    expect(only.split(END)).toHaveLength(2);
  });

  it("defangs escapes even when the frame is off", () => {
    expect(preparePaste("\x1b]0;title\x07", { bracketed: false })).toEqual([
      `${ESC_SYMBOL}]0;title\x07`,
    ]);
  });

  it("keeps a paste at the direct limit in a single write", () => {
    const text = "x".repeat(DIRECT_LIMIT);
    expect(preparePaste(text, { bracketed: false })).toHaveLength(1);
  });

  it("chunks a paste past the direct limit", () => {
    const text = "x".repeat(DIRECT_LIMIT + 1);
    const chunks = preparePaste(text, { bracketed: false });
    expect(chunks.length).toBe(Math.ceil((DIRECT_LIMIT + 1) / CHUNK_SIZE));
    expect(joined(chunks)).toBe(text);
  });

  it("preserves order and content across chunks", () => {
    const text = "abcdefghij";
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 4,
      chunkSize: 3,
    });
    expect(chunks).toEqual(["abc", "def", "ghi", "j"]);
  });

  it("never splits the frame markers across chunks", () => {
    const text = "abcdefghij";
    const chunks = preparePaste(text, {
      bracketed: true,
      directLimit: 4,
      chunkSize: 3,
    });
    // The opener rides entirely on the first chunk, the closer entirely on the
    // last, because the markers are added after the payload is cut.
    expect(chunks[0]).toBe(`${START}abc`);
    expect(chunks[chunks.length - 1]).toBe(`j${END}`);
    expect(chunks.filter((c) => c.includes(START))).toHaveLength(1);
    expect(chunks.filter((c) => c.includes(END))).toHaveLength(1);
    expect(joined(chunks)).toBe(`${START}${text}${END}`);
  });

  it("does not cut between the halves of a surrogate pair", () => {
    // Each chunk is UTF-8 encoded on its own, so a split through the pair
    // would ship two replacement characters instead of the emoji.
    const text = `abc\u{1F600}def`;
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 1,
      chunkSize: 4,
    });
    for (const chunk of chunks) {
      expect(chunk).not.toMatch(/[\uD800-\uDBFF]$/);
      expect(chunk).not.toMatch(/^[\uDC00-\uDFFF]/);
    }
    expect(joined(chunks)).toBe(text);
  });

  it("keeps a surrogate pair whole even when a chunk can hold only one unit", () => {
    const text = `\u{1F600}a`;
    const chunks = preparePaste(text, {
      bracketed: false,
      directLimit: 1,
      chunkSize: 1,
    });
    expect(chunks).toEqual(["\u{1F600}", "a"]);
  });

  it("still frames a paste that had to be chunked", () => {
    const text = "y".repeat(DIRECT_LIMIT + 10);
    const chunks = preparePaste(text, { bracketed: true });
    expect(joined(chunks)).toBe(`${START}${text}${END}`);
  });
});

describe("decideImagePaste", () => {
  it("falls through to text when the backend reports no image", () => {
    expect(decideImagePaste(null, "posix")).toEqual({ kind: "text" });
    expect(decideImagePaste(undefined, "posix")).toEqual({ kind: "text" });
  });

  it("falls through to text for a blank path instead of writing quotes", () => {
    // The 0-byte-PNG bug typed `""` into the shell. A nothing-shaped answer
    // must never become a write, whatever shape the nothing arrives in.
    expect(decideImagePaste("", "posix")).toEqual({ kind: "text" });
    expect(decideImagePaste("   ", "posix")).toEqual({ kind: "text" });
  });

  it("quotes the saved image's path", () => {
    expect(decideImagePaste("C:\Users\John Smith\clip-1.png", "cmd")).toEqual({
      kind: "image",
      write: '"C:\Users\John Smith\clip-1.png"',
    });
  });

  it("types the path with no trailing newline — the user presses Enter", () => {
    const decision = decideImagePaste("/tmp/clip-1.png", "cmd");
    expect(decision.kind).toBe("image");
    expect(decision.kind === "image" && decision.write).toBe('"/tmp/clip-1.png"');
    expect(decision.kind === "image" && decision.write.endsWith("\n")).toBe(false);
  });

  it("quotes for the pane's shell so $ and backticks in the path never expand", () => {
    expect(decideImagePaste("/Users/$USER/`id`/clip.png", "posix")).toEqual({
      kind: "image",
      write: "'/Users/$USER/`id`/clip.png'",
    });
    expect(decideImagePaste("C:\\Users\\$env:X\\clip.png", "powershell")).toEqual({
      kind: "image",
      write: "'C:\\Users\\$env:X\\clip.png'",
    });
  });

  it("falls through to text for a path that cannot be typed safely", () => {
    expect(decideImagePaste("/tmp/a\nb.png", "posix")).toEqual({ kind: "text" });
  });

  // Claude Code strips one outer '…' or "…" from a pasted path and does not
  // un-escape inside it, so a quote in the path must not need an inner escape.
  it("uses double quotes for a path with an apostrophe where that is safe", () => {
    const img = (write: string) => ({ kind: "image", write });
    expect(decideImagePaste("C:\\Users\\O'Brien\\clip.png", "powershell")).toEqual(
      img('"C:\\Users\\O\'Brien\\clip.png"'),
    );
    expect(decideImagePaste("C:\\Users\\O\u2019Brien\\clip.png", "powershell")).toEqual(
      img('"C:\\Users\\O\u2019Brien\\clip.png"'),
    );
    expect(decideImagePaste("C:\\Users\\O'Brien\\clip.png", "cmd")).toEqual(
      img('"C:\\Users\\O\'Brien\\clip.png"'),
    );
    expect(decideImagePaste("/Users/o'brien/clip.png", "posix")).toEqual(
      img('"/Users/o\'brien/clip.png"'),
    );
    expect(decideImagePaste("/Users/o'brien/clip.png", "fish")).toEqual(
      img('"/Users/o\'brien/clip.png"'),
    );
    // No quote in the path: the family's single-quote form, no inner escape.
    expect(decideImagePaste("/Users/me/clip 1.png", "posix")).toEqual(img("'/Users/me/clip 1.png'"));
    // A curly quote is not special to POSIX shells: single quotes stay.
    expect(decideImagePaste("/Users/o\u2019b/clip.png", "posix")).toEqual(
      img("'/Users/o\u2019b/clip.png'"),
    );
  });

  it("refuses an apostrophe path when double quotes would expand something", () => {
    for (const [path, family] of [
      ["C:\\Users\\O'Brien$x\\clip.png", "powershell"],
      ["C:\\Users\\O'Brien`x\\clip.png", "powershell"],
      ["/Users/o'brien/$HOME/clip.png", "posix"],
      ["/Users/o'brien/`id`.png", "posix"],
      ["/Users/o'brien/a\\b.png", "posix"],
      ["/Users/o'brien/!!.png", "posix"],
      ["/Users/o'brien/$(id).png", "fish"],
      ["/Users/me/a\\b.png", "fish"],
      ["/Users/o'brien/clip.png", "unknown"],
    ] as const) {
      expect(decideImagePaste(path, family), `${family} ${path}`).toEqual({ kind: "text" });
    }
  });
});

describe("pasteKeyOwner", () => {
  it("leaves Cmd+V to the native paste command on macOS", () => {
    // The app menu's Edit > Paste turns Cmd+V into a DOM `paste` event; a
    // keydown handler that pasted too was a second copy.
    expect(pasteKeyOwner(true)).toBe("native");
  });

  it("keeps Ctrl+V in the keydown handler elsewhere", () => {
    // WebView2 fires no `paste` event for a cancelled Ctrl+V, so the keydown
    // handler is the only path there.
    expect(pasteKeyOwner(false)).toBe("ymux");
  });
});

/// A DOM `paste` event stand-in that records the order things happen in.
function fakePasteEvent(text: string | null, log: string[]): PasteEventLike {
  return {
    clipboardData:
      text === null
        ? null
        : {
            getData: (type: string) => {
              log.push(`getData:${type}`);
              return type === "text/plain" ? text : "";
            },
          },
    preventDefault: () => log.push("preventDefault"),
    stopImmediatePropagation: () => log.push("stopImmediatePropagation"),
  };
}

describe("claimPasteEvent", () => {
  it("cancels the event and reads its text synchronously", () => {
    const log: string[] = [];
    const text = claimPasteEvent(fakePasteEvent("hello", log));
    expect(text).toBe("hello");
    // All three before returning: `clipboardData` is dead once the handler
    // returns, and an un-cancelled event lets xterm and the IME mirror write.
    expect(log).toEqual(["preventDefault", "stopImmediatePropagation", "getData:text/plain"]);
  });

  it("treats a missing clipboardData as no text", () => {
    const log: string[] = [];
    expect(claimPasteEvent(fakePasteEvent(null, log))).toBe("");
    expect(log).toEqual(["preventDefault", "stopImmediatePropagation"]);
  });
});

function fakeRun(over: Partial<PasteRun> = {}): { run: PasteRun; writes: string[]; errors: unknown[] } {
  const writes: string[] = [];
  const errors: unknown[] = [];
  const run: PasteRun = {
    readImage: async () => null,
    readText: async () => "",
    family: "posix",
    bracketed: () => false,
    canWrite: () => true,
    write: async (data) => {
      writes.push(data);
    },
    reportImageError: (e) => errors.push(e),
    ...over,
  };
  return { run, writes, errors };
}

describe("runPaste", () => {
  it("writes clipboard text exactly once", async () => {
    const { run, writes } = fakeRun({ readText: async () => "echo hi" });
    await runPaste(run);
    expect(writes).toEqual(["echo hi"]);
  });

  it("brackets and normalizes a multi-line paste in one write", async () => {
    const { run, writes } = fakeRun({ readText: async () => "a\nb", bracketed: () => true });
    await runPaste(run);
    expect(writes).toEqual([`${START}a\rb${END}`]);
  });

  it("pastes the image path instead of the text when both are present", async () => {
    let textRead = false;
    const { run, writes } = fakeRun({
      readImage: async () => "/tmp/clip-1.png",
      readText: async () => {
        textRead = true;
        return "text";
      },
    });
    await runPaste(run);
    expect(writes).toEqual(["'/tmp/clip-1.png'"]);
    expect(textRead).toBe(false);
  });

  it("writes nothing when there is neither image nor text", async () => {
    const { run, writes } = fakeRun();
    await runPaste(run);
    expect(writes).toEqual([]);
  });

  it("reports an unsaveable image and does not fall back to text", async () => {
    const boom = new Error("disk full");
    const { run, writes, errors } = fakeRun({
      readImage: async () => {
        throw boom;
      },
      readText: async () => "text",
    });
    await runPaste(run);
    expect(writes).toEqual([]);
    expect(errors).toEqual([boom]);
  });

  it("writes nothing before the PTY is up", async () => {
    const { run, writes } = fakeRun({ readText: async () => "x", canWrite: () => false });
    await runPaste(run);
    expect(writes).toEqual([]);
  });

  it("swallows a denied text read", async () => {
    const { run, writes } = fakeRun({
      readText: async () => {
        throw new Error("denied");
      },
    });
    await runPaste(run);
    expect(writes).toEqual([]);
  });
});

describe("a terminal paste event end to end", () => {
  it("yields exactly one write and nothing for xterm or the IME mirror", async () => {
    const log: string[] = [];
    const ev = fakePasteEvent("ls -la", log);
    const text = claimPasteEvent(ev);
    const { run, writes } = fakeRun({ readText: async () => text });
    await runPaste(run);
    expect(writes).toEqual(["ls -la"]);
    // Cancelled (no native insertion -> no `input` for the IME mirror) and
    // stopped (xterm's own `paste` listener never runs).
    expect(log).toContain("preventDefault");
    expect(log).toContain("stopImmediatePropagation");
  });
});
