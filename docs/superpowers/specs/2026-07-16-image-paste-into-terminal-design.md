# Design: Image Paste into Terminal Panes

**Date:** 2026-07-16
**Target version:** v0.8.20 (or next)
**Status:** Approved for planning

## Motivation

When a CLI agent (e.g. Claude Code) runs inside a ymux terminal pane, there is
no way to hand it a screenshot: a terminal cannot receive a pasted image, only
text. This feature makes **Ctrl+V paste a clipboard image by writing it to a
temp file and typing that file's path into the terminal**, so the CLI can read
the image via the path. The temp files self-clean on a time window.

This reuses two patterns ymux already has:
- the existing **Ctrl+V paste path** in `TerminalPane` (which today reads
  clipboard *text* via `navigator.clipboard.readText()`), and
- the **temp-dir + thin-command + prune** pattern established by the v0.8.19
  scrollback feature (`src-tauri/src/scrollback.rs`).

## Behaviour

In a **terminal pane**, pressing **Ctrl+V**:

1. If the clipboard holds an **image** (`image/png`): ymux saves it to
   `<config-dir>/paste-images/clip-<unix-millis>.png` and types **only the
   absolute file path** into the PTY — no surrounding quotes, no trailing
   newline. The user reviews the path and presses Enter themselves.
2. If the clipboard holds **text** (no image): the existing text-paste
   behaviour is unchanged.
3. If clipboard access fails (permission denied / empty): silent no-op, exactly
   as today.

Temp files older than a configurable retention window (default **24 hours**)
are deleted. Pruning runs on every image paste (cheap; the folder only holds
recent pastes).

## Non-goals

- No native (Rust `arboard`) clipboard reading — we extend the existing
  web-clipboard path (Approach A), adding **zero new Rust dependencies**.
- No image formats other than PNG initially (Windows screenshots expose
  `image/png` through the async clipboard API).
- No Settings-UI toggle initially. The behaviour only changes when the
  clipboard holds an *image*; text paste is untouched, so it is non-intrusive.
  Retention is config-only (no UI) for now.
- Terminal panes only. Browser panes are out of scope (the Ctrl+V handler lives
  in `TerminalPane`).
- No auto-Enter (chosen deliberately as a mistake-guard).

## Architecture

### Frontend — extend `TerminalPane.pasteClipboard()`

`src/terminal/TerminalPane.ts` already intercepts Ctrl+V
(`attachCustomKeyEventHandler`, ~line 176) → `pasteClipboard()` (~line 509).
Rework `pasteClipboard()` to:

The current implementation (verified) is:

```ts
private async pasteClipboard(): Promise<void> {
  try {
    const text = await navigator.clipboard.readText();
    if (text && this.spawned) {
      void api.writePane(this.id, ENCODER.encode(text));
    }
  } catch {
    // Clipboard access denied or empty — silent fail.
  }
}
```

Rework it to try an image first, then keep the exact existing text path:

```ts
private async pasteClipboard(): Promise<void> {
  // Image first: if the clipboard holds a PNG, save it and paste its path.
  try {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      if (item.types.includes("image/png")) {
        const blob = await item.getType("image/png");
        const bytes = new Uint8Array(await blob.arrayBuffer());
        const path = await api.savePasteImage(Array.from(bytes));
        if (path && this.spawned) {
          void api.writePane(this.id, ENCODER.encode(path)); // path only, no newline
        }
        return;
      }
    }
  } catch {
    // clipboard.read() unsupported/denied — continue to the text path below.
  }
  // Existing text-paste path, unchanged.
  try {
    const text = await navigator.clipboard.readText();
    if (text && this.spawned) {
      void api.writePane(this.id, ENCODER.encode(text));
    }
  } catch {
    // Clipboard access denied or empty — silent fail.
  }
}
```

- The PTY write is the existing `api.writePane(this.id, ENCODER.encode(...))`
  gated on `this.spawned` (`ENCODER` is the file's existing `TextEncoder`).
- Bytes cross IPC to the backend as `number[]` (`Array.from(bytes)`); a more
  efficient `Uint8Array`/`ArrayBuffer` transfer may be substituted during
  implementation only if it's cleanly supported, but `number[]` is the default.

### Bridge — `src/ipc/bridge.ts`

Add one wrapper on the `api` object, matching the existing `call(...)` style:

```ts
savePasteImage: (bytes: number[]): Promise<string> =>
  call("save_paste_image", { bytes }),
```

### Backend — new pure module + thin command (mirrors scrollback)

**New ungated module `src-tauri/src/paste_images.rs`** (declared `pub mod
paste_images;` in `lib.rs` **without** a `#[cfg(feature="desktop")]` gate,
std-only, no `tauri` import — so its tests run under Linux CI per CLAUDE.md
rule #1). Mirror `scrollback.rs`'s shape:

```rust
pub fn paste_images_dir() -> PathBuf;               // <config>/ymux/paste-images
fn dir_under(base: &Path) -> ...                    // injectable for hermetic tests
pub fn save_under(base: &Path, millis: u128, bytes: &[u8]) -> io::Result<PathBuf>;
pub fn prune_under(base: &Path, older_than: Duration) -> io::Result<()>;
pub fn save(bytes: &[u8], retention: Duration) -> io::Result<PathBuf>; // public: prune + save under real dir
```

- Filename: `clip-<unix-millis>.png` (millis from `SystemTime::now()`;
  monotonic-enough and sortable; no date-formatting dependency).
- `save` calls `create_dir_all`, prunes files whose `modified()` is older than
  `retention`, writes the PNG via a temp-file + rename (atomic, reusing the
  scrollback write approach), and returns the absolute path.
- Tests (Linux-safe, temp dir via `env::temp_dir()`, like scrollback):
  `save_then_file_exists_and_returns_png_path`, `prune_removes_old_keeps_recent`.

**Thin desktop-gated command in `src-tauri/src/commands.rs`:**

```rust
#[tauri::command]
pub fn save_paste_image(state: State<AppState>, bytes: Vec<u8>) -> YmuxResult<String> {
    let hours = state.config.snapshot().paste_image_retention_hours;  // or however config is read
    let path = crate::paste_images::save(&bytes, Duration::from_secs(hours as u64 * 3600))
        .map_err(YmuxError::Io)?;
    Ok(path.to_string_lossy().into_owned())
}
```

Register in `main.rs`'s `generate_handler!` after the scrollback commands. No
`capabilities/default.json` change (Tauri v2 app-defined commands are not
ACL-gated — verified in the v0.8.19 work).

### Config — `src-tauri/src/config/model.rs`

Add to `Config` (additive, mirrors `persist_scrollback`):

```rust
#[serde(default = "default_paste_image_retention_hours")]
pub paste_image_retention_hours: u32,
```
default = 24. Set in `Config::default()` and every test `Config { .. }` literal.
Add the field to `src/types.ts`'s `Config` interface. **`CONFIG_VERSION` stays
unchanged** (additive serde-default field). This is a `Config` field, not
`PaneSpec`, so no 4-place PaneSpec sync.

## Error handling

| Failure | Behaviour |
|---|---|
| `navigator.clipboard.read()` throws (unsupported/denied) | fall through to text paste |
| No image item, text present | existing text paste |
| No image, no text | silent no-op (as today) |
| `save_paste_image` command errors (fs) | frontend catches → no-op (optionally `console.warn`); do NOT insert a broken path |

## Testing

| Area | Test |
|---|---|
| `paste_images.rs` save | writes a PNG, returns a `.png` path under the dir (temp-dir, Linux-safe) |
| `paste_images.rs` prune | old file removed, recent file kept |
| config | `paste_image_retention_hours` defaults to 24 when absent (mirror `persist_scrollback` test) |
| Linux CI | `cargo check/test --no-default-features --lib --tests -p ymux` green |
| Manual smoke (Windows) | copy a screenshot → Ctrl+V in a terminal pane → path typed (not text); paste text elsewhere → still text; file appears in `paste-images/` and is pruned after retention |

## Compatibility / constraints

- Additive `Config` field with serde default → existing `config.toml` loads
  unchanged; `CONFIG_VERSION` not bumped.
- Zero new dependencies (frontend uses the web clipboard API already in use;
  backend uses only `std`).
- Desktop-gated command behind `#[cfg(feature="desktop")]` via `commands.rs`;
  pure `paste_images.rs` logic stays Linux-testable.
- Version bump handled at release time (target v0.8.20), not part of this
  feature's core work.

## Suggested implementation order

1. `paste_images.rs` pure module + tests (TDD, Linux-safe).
2. `save_paste_image` command + registration + `paste_image_retention_hours`
   config field + types.ts.
3. `bridge.ts` wrapper.
4. `TerminalPane.pasteClipboard()` rework (image-first, text fallback).
5. Manual smoke on Windows.
