# Bottom-Anchored Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a terminal's content is shorter than its pane (normal buffer, viewport at the bottom), draw it against the pane's bottom edge so the prompt sits on the last row and output grows upward. This is presentation only. It is off in the alternate buffer and while the user has scrolled up, and a Settings toggle (`bottom_anchor`, on by default) controls it.

**Architecture:** A pure module `src/terminal/bottomAnchor.ts` works out the anchor offset (in rows) from a structural subset of xterm's `IBuffer`, then turns it into a CSS `translateY`. `TerminalPane` applies that transform to xterm's `.xterm-screen` element, synchronously on `onRender` and coalesced into one animation frame for `onResize` / `onScroll` / `buffer.onBufferChange`. `.pane .xterm` gets `overflow: clip`, so the translated screen never paints outside the pane. The PTY, the xterm buffer, the row count, `FitAddon` and the `.xterm-viewport` scroll element are left alone.

**Tech Stack:** TypeScript, xterm.js 5.5.0 (+ `@xterm/addon-canvas`), `@xterm/headless` 5.5.0 (tests), Vitest, Rust (serde + toml) for the Config field, Tauri 2.

**Spec:** docs/superpowers/specs/2026-09-18-orca-style-layout-design.md (section 3)

## Global Constraints

- No new dependencies. `@xterm/headless` is already a devDependency.
- Features 1 (agent tree) and 2 (yDir dock) land first on this branch. They add a Config field (`agent_tracking`), settings toggles, i18n keys and a right-side dock. **Every shared-file edit below is anchored on an identifier that exists both before and after those features** (`persist_scrollback`, `default_persist_scrollback()`, the `self.persist_scrollback = …` line, the `settings.general.persistScrollback` i18n entry, the `const aboutH` line, the `persistScrollback: () => this.persistScrollback,` option). Never rely on line numbers from this plan for shared files. They are "at time of writing" hints only.
- Config: the new field goes into `merge_layouts_from` (CLAUDE.md memory: a setting missing from there silently reverts on every restart). Do **not** bump `CONFIG_VERSION` (CLAUDE.md rule 8: additive `#[serde(default)]` field).
- Use a plain `bool` with a default fn, not `Option<bool>` (CLAUDE.md rule 3 is about tagged enums; `Config` is not one, but the codebase convention is a default fn: see `default_persist_scrollback`).
- i18n: every user-visible string lives in `src/i18n/i18n.ts` with all 13 languages (en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi).
- Never read xterm private fields (`_core`, `_renderService`). The cell height comes from the public DOM: `.xterm-screen`'s inline `style.height` (set by `CanvasRenderer.ts:101` to `dimensions.css.canvas.height` = `rows × css.cell.height`) divided by `term.rows`.
- TDD for the pure module and the Rust field. DOM application cannot be tested in vitest (`@xterm/headless` has no DOM, and the repo has no jsdom), so the GUI checklist in the final task covers it.
- Commit after every task. Messages end with a blank line then `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.

### Design decisions (verified against `node_modules/@xterm/xterm/src`, v5.5.0)

**1. The `translateY` goes on `.xterm-screen`, not on `.xterm`.** This deviates from the spec's wording ("applies … to the `.xterm` element").

DOM built by `Terminal.open()` (`browser/Terminal.ts:421-496`):

```
.xterm                       ← this.element; wheel + mousedown listeners live here (bindMouse, :603/:720)
├── .xterm-viewport          ← position:absolute; top:0; bottom:0; overflow-y:scroll — the scrollbar and
│   └── .xterm-scroll-area      the element whose scrollTop the wheel writes (Viewport.ts:262, :398)
└── .xterm-screen            ← screenElement (position:relative)
    ├── .xterm-helpers       ← helper <textarea> (:469) + .composition-view (:488) + char measure element
    ├── canvases             ← CanvasRenderer layers (position:absolute; top:0)
    └── decoration container ← BufferDecorationRenderer(screenElement) (:543): search highlights
```

- Pointer mapping works under a transform on either element, so it does not decide the question. `Mouse.ts:getCoordsRelativeToElement` uses `element.getBoundingClientRect()`, and every caller passes `screenElement`: `SelectionService.ts:390/410`, `Linkifier` (constructed with `screenElement`, `Terminal.ts:493`), mouse reporting `getMouseReportCoords(ev, screenElement)` (`:609`), right-click `moveTextAreaUnderMouseCursor(…, screenElement)` / `Clipboard.ts:66`. `getBoundingClientRect` includes transforms on the element and on its ancestors.
- The deciding element is **`.xterm-viewport`**. Translating `.xterm` would slide the scrollbar down too, clip its bottom part, and leave the blank band above the prompt with nothing under the pointer that listens for wheel events (the band would sit outside `.xterm`). Translating `.xterm-screen` keeps the viewport and scrollbar full height and in place. Wheel events over the blank band land on `.xterm-viewport`, which is inside `.xterm`, so xterm's wheel handler still runs.
- Everything that must move with the text is inside `.xterm-screen`: the canvases, the helper textarea (so the OS IME candidate window, positioned from the caret rect, follows), the `.composition-view` preview (`ime.ts` `paint()` copies the textarea's `left/top`, and both elements share the `.xterm-helpers` parent, so the relative coordinates stay consistent), and the search decoration container.
- `.xterm-viewport`'s `scrollTop` math, including `viewportSync.ts`'s `resyncNudge`, is unaffected. A transform does not destroy a layout box, so it does not trigger the scrollTop-reset bug from `reference_xterm_viewport_scrolltop_reset.md`. The GUI checklist confirms this; it is not just asserted.
- The blank band shows `.xterm-viewport`'s background, which xterm sets to the theme background (`Viewport.ts:88`). `TerminalPane.setBgColor` updates that theme, so the band always matches the pane colour.

**2. Clip with `overflow: clip` on `.pane .xterm`.** The spec says "the pane host clips (`overflow: hidden`)". Two changes from that:
- `clip` rather than `hidden`. A transform extends its ancestors' scrollable overflow, and an `overflow: hidden` box is still a programmatic scroll container. A `focus()` without `preventScroll` (`Terminal.ts:534`, reached from the textarea path) or a `scrollIntoView` could scroll it and shift the whole terminal. `overflow: clip` never creates a scroll container. It is supported by WebView2 (Chromium ≥ 90) and WKWebView (Safari ≥ 16).
- On `.xterm` (the tightest box, same width as `.pane__term`) rather than `.pane__term`. The `.search-bar` is a child of `.pane__term`, a sibling of `.xterm`, so it is not clipped either way.

**3. Recompute timing.** The spec says "recomputed on `onRender`, `onResize`, `onScroll`, `onBufferChange`, coalesced per animation frame". `onRender` is fired from inside xterm's own render rAF (`RenderService` → `onRenderedViewportChange` → `Terminal.ts:482`). Deferring it to another rAF would paint one frame of new content at the stale offset: after Enter at a bottom-anchored prompt, the new line would flash below the clip edge. So `onRender` applies **synchronously**. It is already at most once per frame, and it cancels any pending rAF. `onResize` / `onScroll` / `onBufferChange` go through one shared rAF. `onCursorMove` is not subscribed, because every write that moves the cursor also marks rows dirty and so reaches `onRender`. The style is only written when the computed transform string changes, which keeps it off the hot path during a fast build log.

**4. Scan cost is bounded.** `bufferAnchorOffset` checks, in order: alternate buffer → return 0; `viewportY !== baseY` (scrolled up) → return 0. Only then does it scan. The scan runs bottom-up over viewport rows `rows-1 … cursorY+1` and stops at the first non-empty row. Rows at or above the cursor cannot change `max(cursorY, lastContentRow)`, so they are never read. Worst case is `rows - 1 - cursorY` `getLine()` calls, all within the viewport. When the cursor is on the last row (steady state for scrolling output) the cost is zero calls, and a full screen costs one call. A unit test counts the calls. A row holding only background-coloured spaces reads as blank (`translateToString(true)` trims it). That is acceptable.

**5. Frame of reference.** `IBuffer.cursorY` is viewport-relative ("ranges between 0 (when the cursor is at baseY) and rows - 1", `xterm.d.ts:1467-1472`). The scan reads absolute line `baseY + y` and returns the viewport-relative `y`, so both inputs to `anchorOffset` are in the same frame. A headless test with `baseY > 0` pins this down.

**6. Scrollback exists (after `clear`, or after a restore).** Bash `clear` sends `\x1b[3J` (clears scrollback, `baseY` → 0). PowerShell `cls` and ConPTY send `\x1b[2J\x1b[H`, which in xterm.js only resets the viewport rows (`InputHandler.eraseInDisplay` case 2) and leaves scrollback, so `baseY > 0`. In both cases `viewportY === baseY`, the prompt sits on row 0, and the offset is `rows - 1`: the prompt goes to the bottom. The band above it is the viewport's own blank rows translated down. **The transform does not reveal the scrollback above**; the user still scrolls up for it. That is the spec's "blank space above".
- Scrollback restore is untouched by construction. `pendingRestoreReveal`'s `scrollLines(-(rows-2))` makes `viewportY < baseY`, so the offset is 0 and the history is shown exactly as today. Once the user types, xterm scrolls to the bottom (scroll-on-input) and the prompt anchors.
- `resyncViewportScroll`'s `-1`/`+1` nudge pair runs synchronously and settles before the next rAF or render, so the momentary "not at bottom" never paints.

**7. Known, accepted interaction.** If you drag a selection upward past the top edge of a translated screen, xterm's selection auto-scroll kicks in (`SelectionService._getMouseEventScrollAmount` sees a negative y relative to `screenElement`). That scrolls the viewport up, so the offset drops to 0 and the content jumps under the pointer. This is the spec's "off while scrolled up" behaviour. The GUI checklist records it rather than working around it.

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `src/terminal/bottomAnchor.ts` | Create | Pure logic: `anchorOffset`, `lastContentRow`, `bufferAnchorOffset`, `anchorTransform`, the `AnchorBuffer` host interface |
| `src/terminal/bottomAnchor.test.ts` | Create | Vitest: pure cases + real `@xterm/headless` buffers + scan-cost bound |
| `src-tauri/src/config/model.rs` | Modify | `Config.bottom_anchor` + `default_bottom_anchor()` + `Default` + `merge_layouts_from` + tests + full-literal test fixtures |
| `src/types.ts` | Modify | `Config.bottom_anchor: boolean` |
| `src/terminal/TerminalPane.ts` | Modify | `bottomAnchor` option, `.xterm-screen` lookup, event wiring, `refreshBottomAnchor()`, rAF cleanup in `dispose` |
| `src/style.css` | Modify | `.pane .xterm { overflow: clip; }` |
| `src/workspace/WorkspaceManager.ts` | Modify | Pass `bottomAnchor` to panes; `setBottomAnchor` / `bottomAnchor` getter |
| `src/settings/SettingsOverlay.ts` | Modify | "Keep the prompt at the bottom" checkbox row |
| `src/i18n/i18n.ts` | Modify | `settings.general.bottomAnchor` in 13 languages |

---

### Task 1: Pure anchor logic (`bottomAnchor.ts`)

**Files:**
- Create: `src/terminal/bottomAnchor.ts`
- Create: `src/terminal/bottomAnchor.test.ts`

**Interfaces:**
- Consumes: nothing (tests consume `Terminal` from `@xterm/headless`).
- Produces:
  - `interface AnchorInputs { rows: number; cursorY: number; lastContentRow: number; altBuffer: boolean; atBottom: boolean }`
  - `function anchorOffset(i: AnchorInputs): number`
  - `interface AnchorLine { translateToString(trimRight?: boolean): string }`
  - `interface AnchorBuffer { readonly type: "normal" | "alternate"; readonly baseY: number; readonly viewportY: number; readonly cursorY: number; getLine(y: number): AnchorLine | undefined }`
  - `function lastContentRow(buf: AnchorBuffer, rows: number, floor: number): number`
  - `function bufferAnchorOffset(buf: AnchorBuffer, rows: number): number`
  - `function anchorTransform(offset: number, screenHeightPx: number, rows: number): string`

- [ ] **Step 1: Write the failing test**

Create `src/terminal/bottomAnchor.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/headless";
import {
  anchorOffset,
  anchorTransform,
  bufferAnchorOffset,
  lastContentRow,
  type AnchorBuffer,
} from "./bottomAnchor";

function write(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => term.write(data, resolve));
}

function headless(rows = 10): Terminal {
  return new Terminal({ rows, cols: 40, scrollback: 200, allowProposedApi: true });
}

describe("anchorOffset", () => {
  const base = { rows: 10, cursorY: 0, lastContentRow: -1, altBuffer: false, atBottom: true };

  it("pushes a lone prompt on an empty screen down to the last row", () => {
    expect(anchorOffset(base)).toBe(9);
  });

  it("is 0 when the cursor is already on the last row", () => {
    expect(anchorOffset({ ...base, cursorY: 9 })).toBe(0);
  });

  it("is 0 in the alternate buffer", () => {
    expect(anchorOffset({ ...base, altBuffer: true })).toBe(0);
  });

  it("is 0 while the user has scrolled up", () => {
    expect(anchorOffset({ ...base, atBottom: false })).toBe(0);
  });

  it("anchors the lowest content row when content sits below the cursor", () => {
    expect(anchorOffset({ ...base, cursorY: 2, lastContentRow: 6 })).toBe(3);
  });

  it("anchors the cursor row when it is below the last content", () => {
    expect(anchorOffset({ ...base, cursorY: 5, lastContentRow: 3 })).toBe(4);
  });

  it("is 0 for a single-row terminal", () => {
    expect(anchorOffset({ ...base, rows: 1 })).toBe(0);
  });

  it("is never negative", () => {
    expect(anchorOffset({ ...base, cursorY: 12 })).toBe(0);
  });
});

describe("bufferAnchorOffset on a real xterm buffer", () => {
  it("anchors a fresh prompt to the bottom", async () => {
    const term = headless(10);
    await write(term, "$ ");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(9);
    term.dispose();
  });

  it("follows output as it grows", async () => {
    const term = headless(10);
    await write(term, "one\r\ntwo\r\nthree\r\n$ ");
    // Prompt on viewport row 3 → 10 - 1 - 3.
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(6);
    term.dispose();
  });

  it("is 0 once the screen is full", async () => {
    const term = headless(5);
    for (let i = 0; i < 12; i++) await write(term, `line-${i}\r\n`);
    await write(term, "$ ");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });

  it("anchors the prompt again after an ED2 clear that leaves scrollback (baseY > 0)", async () => {
    const term = headless(10);
    for (let i = 0; i < 40; i++) await write(term, `line-${i}\r\n`);
    // PowerShell `cls` / ConPTY: erase display + home, scrollback kept.
    await write(term, "\x1b[2J\x1b[HPS D:\\> ");
    const buf = term.buffer.active;
    expect(buf.baseY).toBeGreaterThan(0);
    expect(buf.viewportY).toBe(buf.baseY);
    expect(buf.cursorY).toBe(0);
    expect(bufferAnchorOffset(buf, term.rows)).toBe(9);
    term.dispose();
  });

  it("reads content rows in the viewport frame, not the absolute frame", async () => {
    const term = headless(10);
    for (let i = 0; i < 40; i++) await write(term, `line-${i}\r\n`);
    await write(term, "\x1b[2J\x1b[H");
    // Content on viewport rows 0..5, then park the cursor on row 1.
    await write(term, "a\r\nb\r\nc\r\nd\r\ne\r\nf\x1b[2;1H");
    const buf = term.buffer.active;
    expect(buf.cursorY).toBe(1);
    // Lowest content row is viewport row 5 → 10 - 1 - 5.
    expect(bufferAnchorOffset(buf, term.rows)).toBe(4);
    term.dispose();
  });

  it("is 0 while scrolled up", async () => {
    const term = headless(5);
    for (let i = 0; i < 20; i++) await write(term, `line-${i}\r\n`);
    await write(term, "\x1b[2J\x1b[H$ ");
    term.scrollLines(-1);
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });

  it("is 0 in the alternate buffer", async () => {
    const term = headless(10);
    await write(term, "$ vim\r\n\x1b[?1049h\x1b[H~");
    expect(term.buffer.active.type).toBe("alternate");
    expect(bufferAnchorOffset(term.buffer.active, term.rows)).toBe(0);
    term.dispose();
  });
});

/// A fake buffer that counts `getLine` calls, to pin the scan-cost bound.
function countingBuffer(
  over: Partial<AnchorBuffer>,
  contentRows: ReadonlySet<number> = new Set(),
): { buf: AnchorBuffer; calls: () => number } {
  let calls = 0;
  const baseY = over.baseY ?? 0;
  const buf: AnchorBuffer = {
    type: "normal",
    baseY,
    viewportY: baseY,
    cursorY: 0,
    ...over,
    getLine(y: number) {
      calls++;
      return { translateToString: () => (contentRows.has(y - baseY) ? "x" : "") };
    },
  };
  return { buf, calls: () => calls };
}

describe("scan cost", () => {
  it("never scans in the alternate buffer", () => {
    const { buf, calls } = countingBuffer({ type: "alternate" });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("never scans while scrolled up", () => {
    const { buf, calls } = countingBuffer({ baseY: 100, viewportY: 90 });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("never scans when the cursor is on the last row", () => {
    const { buf, calls } = countingBuffer({ cursorY: 49 });
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(0);
  });

  it("scans only the rows below the cursor", () => {
    const { buf, calls } = countingBuffer({ baseY: 100, cursorY: 10 });
    expect(bufferAnchorOffset(buf, 50)).toBe(39);
    expect(calls()).toBe(39); // rows 49..11
  });

  it("stops at the first non-empty row from the bottom", () => {
    const { buf, calls } = countingBuffer({ cursorY: 0 }, new Set([49]));
    expect(bufferAnchorOffset(buf, 50)).toBe(0);
    expect(calls()).toBe(1);
  });
});

describe("lastContentRow", () => {
  it("returns -1 when nothing below the floor has content", () => {
    const { buf } = countingBuffer({}, new Set([2]));
    expect(lastContentRow(buf, 10, 2)).toBe(-1);
  });

  it("treats a missing line as blank", () => {
    const buf: AnchorBuffer = {
      type: "normal", baseY: 0, viewportY: 0, cursorY: 0,
      getLine: () => undefined,
    };
    expect(lastContentRow(buf, 10, 0)).toBe(-1);
  });
});

describe("anchorTransform", () => {
  it("is empty for a zero offset", () => {
    expect(anchorTransform(0, 200, 10)).toBe("");
  });

  it("converts rows to CSS pixels via the screen height", () => {
    expect(anchorTransform(3, 200, 10)).toBe("translateY(60px)");
  });

  it("keeps fractional cell heights exact", () => {
    expect(anchorTransform(2, 170, 10)).toBe("translateY(34px)");
    expect(anchorTransform(1, 175, 10)).toBe("translateY(17.5px)");
  });

  it("is empty before the renderer has sized the screen", () => {
    expect(anchorTransform(3, Number.NaN, 10)).toBe("");
    expect(anchorTransform(3, 0, 10)).toBe("");
    expect(anchorTransform(3, 200, 0)).toBe("");
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/terminal/bottomAnchor.test.ts`
Expected: FAIL. Vite reports `Failed to resolve import "./bottomAnchor" from "src/terminal/bottomAnchor.test.ts"` (0 tests run).

- [ ] **Step 3: Write the implementation**

Create `src/terminal/bottomAnchor.ts`:

```ts
/// Bottom-anchored prompt.
///
/// A terminal whose content is shorter than its pane is drawn against the
/// pane's bottom edge, so the prompt sits on the last row and output grows
/// upward. It is purely presentational: the PTY, xterm's buffer and its row
/// count never change. `TerminalPane` translates xterm's `.xterm-screen` down
/// by the offset computed here (see the plan
/// docs/superpowers/plans/2026-09-18-bottom-anchor.md for why that element
/// and not `.xterm`).
///
/// Off (offset 0) in the alternate buffer (vim, less, full-screen TUIs) and
/// while the user has scrolled up. Those programs and that view own the whole
/// screen.

/// Everything is in viewport rows: `0` is the top row on screen, `rows - 1` is
/// the bottom one.
export interface AnchorInputs {
  rows: number;
  /// `buffer.active.cursorY`, which xterm already reports viewport-relative.
  cursorY: number;
  /// Lowest viewport row holding any text, or `-1` for none.
  lastContentRow: number;
  altBuffer: boolean;
  /// `viewportY === baseY`, i.e. the user has not scrolled up.
  atBottom: boolean;
}

/// How many rows to push the screen down. Never negative.
export function anchorOffset(i: AnchorInputs): number {
  if (i.altBuffer || !i.atBottom) return 0;
  return Math.max(0, i.rows - 1 - Math.max(i.cursorY, i.lastContentRow));
}

/// The slice of xterm's `IBufferLine` / `IBuffer` this module reads. Kept
/// structural so both `@xterm/xterm` and `@xterm/headless` buffers satisfy
/// it, and a test can count calls.
export interface AnchorLine {
  translateToString(trimRight?: boolean): string;
}

export interface AnchorBuffer {
  readonly type: "normal" | "alternate";
  readonly baseY: number;
  readonly viewportY: number;
  readonly cursorY: number;
  getLine(y: number): AnchorLine | undefined;
}

/// Lowest viewport row strictly below `floor` that holds any text, or `-1`.
///
/// Scans bottom-up and stops at the first hit, so a full screen costs one
/// `getLine`. Rows at or above `floor` are never read. A row of
/// background-coloured spaces counts as blank (`trimRight`).
export function lastContentRow(buf: AnchorBuffer, rows: number, floor: number): number {
  for (let y = rows - 1; y > floor; y--) {
    const line = buf.getLine(buf.baseY + y);
    if (line !== undefined && line.translateToString(true).length > 0) return y;
  }
  return -1;
}

/// `anchorOffset` for a live buffer. The cheap checks go first so the scan is
/// unreachable in the alternate buffer and while scrolled up. The scan is
/// bounded to the `rows - 1 - cursorY` rows below the cursor: nothing at or
/// above the cursor can change `max(cursorY, lastContentRow)`.
export function bufferAnchorOffset(buf: AnchorBuffer, rows: number): number {
  const altBuffer = buf.type === "alternate";
  const atBottom = buf.viewportY === buf.baseY;
  if (altBuffer || !atBottom) return 0;
  const cursorY = buf.cursorY;
  return anchorOffset({
    rows,
    cursorY,
    lastContentRow: lastContentRow(buf, rows, cursorY),
    altBuffer,
    atBottom,
  });
}

/// CSS `transform` for an offset of `offset` rows. `screenHeightPx` is
/// `.xterm-screen`'s inline height, which the renderer sets to exactly
/// `rows × cell height`, so dividing gives the cell height without touching
/// xterm internals. Empty string (no transform) for a zero offset or before
/// the renderer has sized the screen.
export function anchorTransform(offset: number, screenHeightPx: number, rows: number): string {
  if (offset <= 0 || rows <= 0 || !(screenHeightPx > 0)) return "";
  return `translateY(${(offset * screenHeightPx) / rows}px)`;
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/terminal/bottomAnchor.test.ts`
Expected: PASS. `Test Files  1 passed (1)`, `Tests  26 passed (26)`.

Run: `npx tsc --noEmit`
Expected: no output, exit 0. This also proves that `@xterm/headless`'s `IBuffer` is assignable to `AnchorBuffer`.

- [ ] **Step 5: Commit**

```bash
git add src/terminal/bottomAnchor.ts src/terminal/bottomAnchor.test.ts
git commit -m "$(cat <<'EOF'
feat(terminal): pure bottom-anchor offset for short terminal content

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `bottom_anchor` Config field (Rust + TS type)

**Files:**
- Modify: `src-tauri/src/config/model.rs`: `Config` struct (the `persist_scrollback` field, ~line 49-50), default fns (after `default_persist_scrollback`, ~line 80-82), `impl Default for Config` (~line 96-111), `merge_layouts_from` (~line 142-160), `mod tests` (the `persist_scrollback_defaults_true_when_absent` test ~line 663, `merge_layouts_carries_user_settings_back` ~line 698, and every full `Config { … }` literal).
- Modify: `src/types.ts`: `interface Config` (the `persist_scrollback: boolean;` line, ~line 62).

**Interfaces:**
- Consumes: nothing.
- Produces:
  - Rust: `pub bottom_anchor: bool` on `Config`, `#[serde(default = "default_bottom_anchor")]`, `fn default_bottom_anchor() -> bool { true }`.
  - TS: `Config.bottom_anchor: boolean`.

- [ ] **Step 1: Write the failing tests**

In `src-tauri/src/config/model.rs`, inside `mod tests`, add these two tests immediately after the `persist_scrollback_defaults_true_when_absent` test:

```rust
    #[test]
    fn bottom_anchor_defaults_true_when_absent() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert!(parsed.bottom_anchor);
    }

    #[test]
    fn bottom_anchor_roundtrips_false() {
        let config = Config {
            bottom_anchor: false,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert!(!loaded.bottom_anchor);
    }
```

In the existing `merge_layouts_carries_user_settings_back` test, whatever features 1/2 already added to it, make two additions and rewrite nothing else:
1. Inside the `let frontend_save = Config { … }` literal, add `bottom_anchor: false,` on the line right after `persist_scrollback: false,`.
2. After the `assert!(!backend.persist_scrollback);` line, add:

```rust
        assert!(!backend.bottom_anchor);
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --no-default-features --lib -p ymux -- bottom_anchor`
Expected: compile FAIL with `error[E0560]: struct `Config` has no field named `bottom_anchor`` and `error[E0609]: no field `bottom_anchor` on type `Config``.

- [ ] **Step 3: Implement the field**

In `src-tauri/src/config/model.rs`:

(a) In `pub struct Config`, immediately after

```rust
    #[serde(default = "default_persist_scrollback")]
    pub persist_scrollback: bool,
```

add:

```rust
    /// Draw a terminal whose content is shorter than its pane against the
    /// pane's bottom edge, so the prompt sits on the last row. Presentation
    /// only (the frontend's `bottomAnchor.ts`). Additive with a serde default,
    /// so no `CONFIG_VERSION` bump.
    #[serde(default = "default_bottom_anchor")]
    pub bottom_anchor: bool,
```

(b) Immediately after `fn default_persist_scrollback() -> bool { true }`, add:

```rust
fn default_bottom_anchor() -> bool {
    true
}
```

(c) In `merge_layouts_from`, immediately after `self.persist_scrollback = incoming.persist_scrollback;`, add:

```rust
        self.bottom_anchor = incoming.bottom_anchor;
```

(d) Every **full** `Config { … }` literal (one without `..Config::default()`) now fails with `E0063 missing field `bottom_anchor``. That covers `impl Default for Config` and the test fixtures. Each of them contains a `persist_scrollback: true,` line. Add `bottom_anchor: true,` directly after it, with the same indentation, in one pass:

```bash
sed -i -E 's/^([[:space:]]*)persist_scrollback: true,$/&\n\1bottom_anchor: true,/' src-tauri/src/config/model.rs
```

Then verify that the number of inserted lines equals the number of `persist_scrollback: true,` lines. At time of writing both are 9: `Default` plus 8 test literals near lines 766, 778, 836, 868, 1003, 1032, 1094, 1227. Features 1/2 may have added more literals, which the sed handles too.

```bash
grep -c "persist_scrollback: true," src-tauri/src/config/model.rs
grep -c "bottom_anchor: true," src-tauri/src/config/model.rs
```

Expected: the two counts are equal. The `merge_layouts_carries_user_settings_back` literal uses `persist_scrollback: false,` plus `..Config::default()`, so the sed leaves it alone and Step 1 already covered it.

(e) In `src/types.ts`, `interface Config`, immediately after `persist_scrollback: boolean;`, add:

```ts
  /// Draw short terminal content against the pane's bottom edge (the prompt
  /// sits on the last row). See `src/terminal/bottomAnchor.ts`.
  bottom_anchor: boolean;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --no-default-features --lib -p ymux -- bottom_anchor`
Expected: `test result: ok. 2 passed; 0 failed` (`bottom_anchor_defaults_true_when_absent`, `bottom_anchor_roundtrips_false`).

Run: `cargo test --no-default-features --lib -p ymux -- merge_layouts`
Expected: `test result: ok.` with all `merge_layouts_*` tests passing, including `merge_layouts_carries_user_settings_back`.

Run: `cargo test --no-default-features --lib -p ymux`
Expected: `test result: ok.` with 0 failed. Every fixture literal compiles.

Run: `cargo fmt --all --check` and `npx tsc --noEmit`
Expected: no output, exit 0 for both. Nothing in `src/` builds a `Config` object literal, so the new required TS field breaks nothing. The frontend only receives `Config` from the backend, which always serializes the field.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config/model.rs src/types.ts
git commit -m "$(cat <<'EOF'
feat(config): bottom_anchor setting, default on, merged on save

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Apply the anchor in `TerminalPane` + clip in CSS

**Files:**
- Modify: `src/terminal/TerminalPane.ts`: imports (~line 20-25), `TerminalPaneOptions` (after `persistScrollback?` ~line 59-62), private fields (after `viewportEl` ~line 88-91), constructor (right after `this.term.loadAddon(this.serializeAddon);` ~line 310), new methods (after `resyncViewportScroll()` ~line 716-726), `dispose()` (~line 800-822).
- Modify: `src/style.css`: the `.pane .xterm { … }` rule (~line 189-193).

**Interfaces:**
- Consumes: `bufferAnchorOffset(buf: AnchorBuffer, rows: number): number` and `anchorTransform(offset: number, screenHeightPx: number, rows: number): string` from Task 1.
- Produces:
  - `TerminalPaneOptions.bottomAnchor?: () => boolean`, a live getter (same pattern as `persistScrollback`), so flipping the setting needs no pane rebuild.
  - `TerminalPane.refreshBottomAnchor(): void`, which recomputes and applies now (called by the owner when the setting flips).

- [ ] **Step 1: Add the import**

In `src/terminal/TerminalPane.ts`, immediately after `import { resyncNudge } from "./viewportSync";`, add:

```ts
import { anchorTransform, bufferAnchorOffset } from "./bottomAnchor";
```

- [ ] **Step 2: Add the option**

In `interface TerminalPaneOptions`, immediately after the `persistScrollback?: () => boolean;` member, add:

```ts
  /// Whether the bottom-anchored prompt is enabled (read live, like
  /// `persistScrollback`). Absent means off, so a standalone pane renders
  /// exactly like plain xterm.
  bottomAnchor?: () => boolean;
```

- [ ] **Step 3: Add the fields**

Immediately after the `private viewportEl: HTMLElement | null = null;` field (and its doc comment), add:

```ts
  /// xterm's `.xterm-screen`, the element the bottom anchor translates.
  /// Resolved once after `term.open()`. It holds the canvases, the helper
  /// textarea / IME preview and the decoration layer, but not the
  /// `.xterm-viewport` scroll element, which must stay put so the scrollbar
  /// and wheel keep working. See `bottomAnchor.ts`.
  private screenEl: HTMLElement | null = null;
  /// rAF coalescing the non-render triggers of `applyBottomAnchor`.
  private pendingAnchorRaf = 0;
  /// Last `transform` written to `screenEl`, so an unchanged offset costs no
  /// style write on every rendered frame.
  private appliedAnchor = "";
```

- [ ] **Step 4: Wire the events**

In the constructor, immediately after `this.term.loadAddon(this.serializeAddon);`, add:

```ts
    // Bottom-anchored prompt. `onRender` fires from inside xterm's own render
    // frame, so applying there lands in the same paint as the new content.
    // Deferring it would show one frame of fresh output below the clip edge.
    // The other triggers share one rAF.
    this.screenEl =
      this.term.element?.querySelector<HTMLElement>(".xterm-screen") ?? null;
    this.term.onRender(() => this.applyBottomAnchor());
    this.term.onResize(() => this.scheduleBottomAnchor());
    this.term.onScroll(() => this.scheduleBottomAnchor());
    this.term.buffer.onBufferChange(() => this.scheduleBottomAnchor());
```

(These subscriptions are owned by `this.term` and disposed by `this.term.dispose()`. The existing `onResize` handler that resizes the PTY stays as it is. This is a second, independent listener.)

- [ ] **Step 5: Add the methods**

Immediately after the `resyncViewportScroll(): void { … }` method, add:

```ts
  /// Recompute and apply the bottom anchor now. The owner calls this when the
  /// setting flips; turning it off clears the transform.
  refreshBottomAnchor(): void {
    this.applyBottomAnchor();
  }

  private scheduleBottomAnchor(): void {
    if (this.pendingAnchorRaf) return;
    this.pendingAnchorRaf = requestAnimationFrame(() => {
      this.pendingAnchorRaf = 0;
      this.applyBottomAnchor();
    });
  }

  /// Translate `.xterm-screen` down so short content sits on the pane's last
  /// row. Purely visual: pointer mapping follows because xterm measures
  /// `screenElement.getBoundingClientRect()`, which includes the transform.
  private applyBottomAnchor(): void {
    if (this.pendingAnchorRaf) {
      cancelAnimationFrame(this.pendingAnchorRaf);
      this.pendingAnchorRaf = 0;
    }
    const screen = this.screenEl;
    if (!screen) return;
    const rows = this.term.rows;
    const offset = this.opts.bottomAnchor?.()
      ? bufferAnchorOffset(this.term.buffer.active, rows)
      : 0;
    const transform = anchorTransform(offset, parseFloat(screen.style.height), rows);
    if (transform === this.appliedAnchor) return;
    this.appliedAnchor = transform;
    screen.style.transform = transform;
  }
```

- [ ] **Step 6: Cancel the rAF on dispose**

In `dispose(permanent = false)`, immediately after the `window.removeEventListener("beforeunload", this.flushScrollbackOnUnload);` line, add:

```ts
    if (this.pendingAnchorRaf) cancelAnimationFrame(this.pendingAnchorRaf);
```

- [ ] **Step 7: Clip the translated screen**

In `src/style.css`, replace the existing rule

```css
.pane .xterm {
  flex: 1 1 auto;
  height: 100%;
  width: 100%;
}
```

with

```css
.pane .xterm {
  flex: 1 1 auto;
  height: 100%;
  width: 100%;
  /* The bottom-anchored prompt translates `.xterm-screen` down (see
     src/terminal/bottomAnchor.ts), so its blank lower rows overflow this box.
     `clip`, not `hidden`: a hidden box is still a scroll container that a
     focus() or scrollIntoView could scroll. A clip box never scrolls. */
  overflow: clip;
}
```

- [ ] **Step 8: Verify types and unit tests**

Run: `npx tsc --noEmit`
Expected: no output, exit 0.

Run: `npx vitest run src/terminal`
Expected: all `src/terminal/*.test.ts` files pass (bottomAnchor, dropPaths, ime, paneStatus, restoreGuard, scrollbackPersist, viewportSync), 0 failed.

(No owner passes `bottomAnchor` yet, so the app renders unchanged until Task 4. That keeps this commit safe on its own.)

- [ ] **Step 9: Commit**

```bash
git add src/terminal/TerminalPane.ts src/style.css
git commit -m "$(cat <<'EOF'
feat(terminal): translate .xterm-screen to bottom-anchor short content

Applied on onRender (same frame) and coalesced on resize/scroll/buffer
change; the viewport scroll element is left untouched so the scrollbar,
wheel and scrollTop resync keep working. .xterm clips with overflow: clip.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Setting plumbing, Settings toggle, i18n

**Files:**
- Modify: `src/workspace/WorkspaceManager.ts`: the `new TerminalPane({ … })` options in the terminal-pane factory (the `persistScrollback: () => this.persistScrollback,` line, ~line 455), and the settings accessors (after `get persistScrollback()`, ~line 899-901).
- Modify: `src/settings/SettingsOverlay.ts`: the General section, immediately before `const aboutH = document.createElement("h4");` (~line 307).
- Modify: `src/i18n/i18n.ts`: immediately after the `"settings.general.persistScrollback": { … },` entry (~line 76-81).

**Interfaces:**
- Consumes: `TerminalPaneOptions.bottomAnchor?: () => boolean`, `TerminalPane.refreshBottomAnchor(): void` (Task 3); `Config.bottom_anchor: boolean` (Task 2).
- Produces:
  - `WorkspaceManager.setBottomAnchor(enabled: boolean): void`
  - `WorkspaceManager.bottomAnchor: boolean` (getter)
  - i18n key `settings.general.bottomAnchor`

- [ ] **Step 1: Add the i18n key**

In `src/i18n/i18n.ts`, immediately after the closing `},` of the `"settings.general.persistScrollback"` entry, add:

```ts
  "settings.general.bottomAnchor": {
    en: "Keep the prompt at the bottom of the pane", ko: "프롬프트를 패널 아래쪽에 고정", ja: "プロンプトをペインの下端に固定",
    zh: "将提示符固定在窗格底部", hi: "प्रॉम्प्ट को पेन के नीचे रखें", es: "Mantener el prompt en la parte inferior del panel",
    fr: "Garder l'invite en bas du volet", ar: "إبقاء موجّه الأوامر أسفل اللوحة", pt: "Manter o prompt na parte inferior do painel",
    ru: "Держать приглашение внизу панели", tr: "İstemi bölmenin altında tut", de: "Eingabeaufforderung am unteren Rand halten", vi: "Giữ dấu nhắc ở cuối khung",
  },
```

- [ ] **Step 2: Pass the live getter to every terminal pane**

In `src/workspace/WorkspaceManager.ts`, in the `new TerminalPane({ … })` call, immediately after `persistScrollback: () => this.persistScrollback,`, add:

```ts
      bottomAnchor: () => this.bottomAnchor,
```

- [ ] **Step 3: Add the setter and getter**

In `src/workspace/WorkspaceManager.ts`, immediately after the `get persistScrollback(): boolean { … }` getter, add:

```ts
  /// Enable/disable the bottom-anchored prompt and persist the choice. Applies
  /// to every live terminal in every workspace at once, for the same reason
  /// `setFontSize` does: hidden workspaces keep their panes alive.
  setBottomAnchor(enabled: boolean): void {
    this.config.bottom_anchor = enabled;
    for (const cache of this.paneCaches.values()) {
      for (const pane of cache.values()) {
        if (pane instanceof TerminalPane) pane.refreshBottomAnchor();
      }
    }
    this.persistDebounced();
  }

  get bottomAnchor(): boolean {
    return this.config.bottom_anchor;
  }
```

- [ ] **Step 4: Add the Settings toggle**

In `src/settings/SettingsOverlay.ts`, immediately before `const aboutH = document.createElement("h4");`, add (same shape as the `scrollbackRow` block):

```ts
    const anchorRow = document.createElement("div");
    anchorRow.className = "settings-row";
    const anchorLabel = document.createElement("div");
    anchorLabel.className = "settings-row__label";
    anchorLabel.textContent = t("settings.general.bottomAnchor");
    anchorRow.appendChild(anchorLabel);
    const anchorToggle = document.createElement("input");
    anchorToggle.type = "checkbox";
    anchorToggle.checked = manager.bottomAnchor;
    anchorToggle.addEventListener("change", () => {
      manager.setBottomAnchor(anchorToggle.checked);
    });
    anchorRow.appendChild(anchorToggle);
    const anchorSpacer = document.createElement("div");
    anchorRow.appendChild(anchorSpacer);
    host.appendChild(anchorRow);
```

(If features 1/2 already put their own rows before `aboutH`, this row goes after them, which is fine. The overlay's `onLangChange` handler calls `renderCurrentSection()`, which rebuilds `renderGeneral`, so the label re-translates without extra code.)

- [ ] **Step 5: Verify**

Run: `npx tsc --noEmit`
Expected: no output, exit 0.

Run: `npx vitest run`
Expected: all test files pass, 0 failed.

- [ ] **Step 6: Commit**

```bash
git add src/workspace/WorkspaceManager.ts src/settings/SettingsOverlay.ts src/i18n/i18n.ts
git commit -m "$(cat <<'EOF'
feat(settings): toggle for the bottom-anchored prompt

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Full verification + manual GUI checklist

**Files:** none modified. (If a check fails, fix it in the file the failing check points at, and commit that fix separately.)

**Interfaces:**
- Consumes: everything above.
- Produces: verification evidence.

- [ ] **Step 1: Automated suite**

Run: `pnpm test` (= `bash scripts/test.sh`: fmt check, tsc, vitest, clippy for tools and ymux lib, cargo tests)
Expected: ends with `✓ All checks passed.`

Run: `cargo clippy --workspace -- -D warnings` (Windows/macOS only, since it pulls in the desktop feature)
Expected: `Finished` with no warnings.

Run: `cargo check --no-default-features --lib --tests -p ymux`
Expected: `Finished`, no errors (the Linux-safe gate from CLAUDE.md rule 1).

- [ ] **Step 2: Manual GUI checklist (`pnpm tauri dev`)**

Tick each only after seeing it. Where the setting is involved, test with it **on** (the default) unless the item says otherwise.

- [ ] **Fresh pane:** open a new pane (pwsh, cmd, Git Bash). The prompt is on the pane's last row, and the band above is blank in the pane's background colour (also after changing the pane bg colour from the HotKey bar ⚙).
- [ ] **Output grows upward:** run `dir` / `ls` a few times. Each new line pushes the content up and the prompt stays on the last row. Once the screen is full, behaviour is identical to plain xterm. No one-frame flash of the new line below the pane edge on Enter.
- [ ] **clear:** `cls` in pwsh (ED2, scrollback kept) and `clear` in Git Bash (ED3). The prompt goes back to the last row with blank space above. Scrolling up with the wheel after `cls` shows the earlier history (the anchor turns off while scrolled up), and scrolling back to the bottom re-anchors.
- [ ] **Wheel scroll:** the wheel works over the text **and over the blank band**. The scrollbar is full height, not clipped at the bottom, and never jumps to the top of the scrollback, including after a workspace switch, a split, a zoom toggle (`Ctrl+Shift+Z`), and a gutter drag (viewportSync regression).
- [ ] **Selection:** drag-select a word on the anchored rows. The highlighted cells are exactly the ones under the pointer, and copy (context menu → Copy) yields that text. Double-click selects the word under the pointer. Known and accepted: dragging upward past the top of the text auto-scrolls into scrollback, the anchor turns off, and the text jumps.
- [ ] **Link hover:** hover a URL on an anchored row. The underline is on the URL, and Ctrl+click (Cmd+click on macOS) opens it.
- [ ] **Right-click:** the context menu opens and Paste lands at the prompt.
- [ ] **IME preview (Windows, WebView2):** type Korean (`안녕하세요`) at an anchored prompt. The `.composition-view` preview sits on the prompt row at the caret, and the OS candidate window appears next to it rather than at the unshifted position.
- [ ] **IME (macOS, WKWebView):** the same test. Syllables commit intact, and the candidate/marked-text position follows the caret.
- [ ] **Search highlight:** `Ctrl+F`, search a word visible on an anchored row. The highlight decoration covers the word itself, and the search bar is not clipped. Searching a word only in scrollback scrolls up, and the view un-anchors correctly.
- [ ] **Resize:** resize the window, drag a split gutter, and change font size (`Ctrl+=` / `Ctrl+-` / `Ctrl+0`). The prompt stays on the last row after each change, with no gap or overlap at the bottom edge. In a vertical split, the top pane's translated screen never paints into the pane below (clip).
- [ ] **vim / alt buffer:** `vim` (Git Bash) or `less`. The full-screen app fills the pane from the top (offset 0). On exit, the prompt re-anchors.
- [ ] **Claude Code (normal-buffer TUI):** run `claude` with little history. Its input box sits at the bottom of the pane, and typing and redraws don't garble.
- [ ] **Scrollback restore:** with "Persist terminal scrollback" on, type something in a pane, restart the app. The restored history is visible on open (reveal scroll, anchor off). After typing a key, the view returns to the bottom and the prompt anchors.
- [ ] **Hidden workspace:** switch away from a workspace and back. Anchored panes are still anchored, not reset to the top.
- [ ] **Toggle:** Settings → General → "Keep the prompt at the bottom of the pane" off. Every open pane (including panes in other workspaces) immediately draws from the top. Turn it back on and they re-anchor. Restart the app: the choice persisted (merge_layouts_from).
- [ ] **i18n:** switch the UI language to ko and ja. The toggle label is translated.
