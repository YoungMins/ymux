# Image Paste into Terminal Panes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Ctrl+V in a ymux terminal pane paste a clipboard image by saving it to a self-cleaning temp dir and typing the file's path into the PTY, so an in-pane CLI (e.g. Claude Code) can read the image via that path.

**Architecture:** Extend the existing web-clipboard paste path in `TerminalPane` (which already reads clipboard *text*) to also handle images: read the PNG via `navigator.clipboard.read()`, hand the bytes to a thin Rust command that writes the file, and type back the returned path. The backend mirrors the v0.8.19 scrollback pattern: a pure, ungated, Linux-testable module (`paste_images.rs`) plus a thin desktop-gated command. Zero new dependencies.

**Tech Stack:** Rust (Tauri 2, std-only backend logic), TypeScript (xterm.js + Web Clipboard API), TOML config.

## Global Constraints

- **Zero new dependencies** — frontend uses the Web Clipboard API already in use for text paste; backend uses only `std` + the already-present `dirs`/`uuid` crates.
- **Linux CI must pass** — `cargo check/test --no-default-features --lib --tests -p ymux` (CLAUDE.md rule #1). Pure logic goes in an **ungated** `paste_images.rs` (no `#[cfg(feature="desktop")]`, no `tauri` import) so its tests run on Linux; the `#[tauri::command]` wrapper stays in the desktop-gated `commands.rs`.
- **`CONFIG_VERSION` stays unchanged** — the new `paste_image_retention_hours` config field is additive with a serde default (CLAUDE.md rule #8). It is a `Config` field, **not** `PaneSpec`, so no 4-place PaneSpec sync.
- **No `capabilities/default.json` change** — Tauri v2 app-defined `generate_handler!` commands are not ACL-gated (verified in the v0.8.19 work; no existing custom command has an entry).
- **Do NOT touch `src-tauri/Cargo.toml`** — it carries a pre-existing unrelated LF/CRLF working-tree flag; leave it unstaged.
- **Behaviour is additive** — text paste is unchanged; the new path only triggers when the clipboard holds an `image/png`. PNG only, terminal panes only, no auto-Enter, no Settings-UI toggle (YAGNI).
- **Version bump is deferred to release** (target v0.8.20), not part of this plan.
- Commit after every task. DRY, YAGNI, TDD.

## File Structure

- Create: `src-tauri/src/paste_images.rs` — pure std-only save + time-based prune (mirrors `scrollback.rs`); Linux-testable.
- Modify: `src-tauri/src/lib.rs` — `pub mod paste_images;` (ungated).
- Modify: `src-tauri/src/config/model.rs` — `Config.paste_image_retention_hours: u32` (default 24) + round-trip default test.
- Modify: `src-tauri/src/commands.rs` — thin desktop-gated `#[tauri::command] save_paste_image`.
- Modify: `src-tauri/src/main.rs` — register `save_paste_image` in `generate_handler!`.
- Modify: `src/types.ts` — `paste_image_retention_hours` on the `Config` interface.
- Modify: `src/ipc/bridge.ts` — `savePasteImage` wrapper on the `api` object.
- Modify: `src/terminal/TerminalPane.ts` — rework `pasteClipboard()` (image-first, existing text path preserved).

Key existing anchors (verified against the current tree at commit e691015):
- `src-tauri/src/scrollback.rs` — the exact pure-module shape to mirror (`*_dir()`, `*_under(base,...)`, temp-file+rename atomic write, hermetic `tempdir()` tests).
- `src-tauri/src/commands.rs:228` `save_scrollback(pane_id, blob)` — the thin-command style; and `state.config.snapshot()` (used at :70/:77/:118) is how a command reads a `Config` field.
- `src-tauri/src/config/model.rs` — `persist_scrollback` (field + `default_persist_scrollback` + `persist_scrollback_defaults_true_when_absent` test) is the field pattern to mirror.
- `src/terminal/TerminalPane.ts:509` `pasteClipboard()` — currently `navigator.clipboard.readText()` → `api.writePane(this.id, ENCODER.encode(text))` gated on `this.spawned`.
- `src/ipc/bridge.ts` — `saveScrollback: (id, blob) => call("save_scrollback", {...})` is the wrapper style to mirror.

---

## Task 1: `paste_images.rs` — pure save + time-based prune

**Files:**
- Create: `src-tauri/src/paste_images.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod paste_images;`, ungated)

**Interfaces:**
- Produces:
  - `pub fn paste_images_dir() -> std::path::PathBuf`
  - `pub fn save(bytes: &[u8], retention: std::time::Duration) -> std::io::Result<std::path::PathBuf>` — prune old files, then write `clip-<unix-millis>.png`, return its absolute path
  - (internal, tested) `file_under(base, millis) -> PathBuf`, `parse_millis(name: &str) -> Option<u128>`, `save_under(base, millis, bytes) -> io::Result<PathBuf>`, `prune_under(base, now_millis, retention_millis) -> io::Result<()>`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/paste_images.rs` with only the test module (implementation comes next):

```rust
//! Pure, `std`-only logic for saving pasted clipboard images to disk and
//! time-pruning old ones. Deliberately free of any Tauri dependency so it
//! compiles and its tests run under `cargo test --no-default-features --lib
//! -p ymux` on Linux CI, unlike `commands.rs` (gated behind `desktop`). The
//! `#[tauri::command]` wrapper in `commands.rs` calls `save` and maps
//! `std::io::Error` to `YmuxError::Io`.
//!
//! Files are named `clip-<unix-millis>.png`; pruning parses that embedded
//! timestamp rather than the filesystem mtime, so it is deterministic and
//! testable without touching file times.

use std::path::{Path, PathBuf};

// ---- implementation goes here in Step 3 ----

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-paste-images-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    #[test]
    fn save_under_writes_png_and_returns_path() {
        let base = tempdir();
        let bytes = b"\x89PNG\r\n\x1a\nfake-png-bytes";
        let path = save_under(&base, 1234, bytes).expect("save_under should succeed");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("clip-1234.png"));
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parse_millis_reads_valid_and_rejects_other() {
        assert_eq!(parse_millis("clip-1710000000000.png"), Some(1710000000000));
        assert_eq!(parse_millis("clip-0.png"), Some(0));
        assert_eq!(parse_millis("clip-abc.png"), None);
        assert_eq!(parse_millis("notes.txt"), None);
        assert_eq!(parse_millis("clip-123.txt"), None);
    }

    #[test]
    fn prune_removes_old_keeps_recent() {
        let base = tempdir();
        save_under(&base, 1000, b"old").expect("save old");
        save_under(&base, 9000, b"recent").expect("save recent");
        // now = 10000, retention = 2000ms: 10000-1000=9000 > 2000 (drop),
        // 10000-9000=1000 < 2000 (keep).
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(!file_under(&base, 1000).exists(), "old file should be pruned");
        assert!(file_under(&base, 9000).exists(), "recent file should remain");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_ignores_non_clip_files() {
        let base = tempdir();
        std::fs::write(base.join("keepme.txt"), b"x").unwrap();
        save_under(&base, 1000, b"old").expect("save old");
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(base.join("keepme.txt").exists(), "unrelated files must be left alone");
        let _ = std::fs::remove_dir_all(&base);
    }
}
```

Add the module declaration to `src-tauri/src/lib.rs`, next to `pub mod scrollback;` (ungated — no `#[cfg(...)]`):

```rust
pub mod paste_images;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --no-default-features --lib -p ymux paste_images::`
Expected: FAIL to compile — `save_under`, `parse_millis`, `prune_under`, `file_under` are undefined.

- [ ] **Step 3: Write the implementation**

Insert above the `#[cfg(test)]` module in `src-tauri/src/paste_images.rs`:

```rust
/// Directory pasted images are written to: `<config_dir>/ymux/paste-images`,
/// falling back to a relative directory if the OS config dir can't be
/// determined (mirrors `scrollback::scrollback_dir`).
pub fn paste_images_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("paste-images"))
        .unwrap_or_else(|| PathBuf::from("./ymux-paste-images"))
}

/// Path to the image file for a given millisecond timestamp under `base`.
fn file_under(base: &Path, millis: u128) -> PathBuf {
    base.join(format!("clip-{millis}.png"))
}

/// Parse the embedded millisecond timestamp out of a `clip-<millis>.png` file
/// name. Returns `None` for any name that doesn't match that exact shape.
fn parse_millis(name: &str) -> Option<u128> {
    name.strip_prefix("clip-")
        .and_then(|s| s.strip_suffix(".png"))
        .and_then(|s| s.parse::<u128>().ok())
}

/// Write `bytes` as `clip-<millis>.png` under `base` (created if needed) via a
/// temp file + rename so a crash mid-write can't leave a truncated image,
/// mirroring `scrollback::save_blob_under`. Returns the final path.
fn save_under(base: &Path, millis: u128, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(base)?;
    let path = file_under(base, millis);
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Delete every `clip-<millis>.png` in `base` whose embedded timestamp is
/// older than `retention_millis` relative to `now_millis`. Non-matching files
/// are left untouched. A per-file removal error is ignored so one locked file
/// can't abort pruning the rest.
fn prune_under(base: &Path, now_millis: u128, retention_millis: u128) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(millis) = parse_millis(name) {
            if now_millis.saturating_sub(millis) > retention_millis {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Prune old pasted images, then save `bytes` as a new `clip-<now>.png` under
/// the real OS paste-images directory, returning its absolute path. `retention`
/// is how long a pasted image is kept before it becomes eligible for pruning.
pub fn save(bytes: &[u8], retention: std::time::Duration) -> std::io::Result<PathBuf> {
    let dir = paste_images_dir();
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = prune_under(&dir, now_millis, retention.as_millis());
    save_under(&dir, now_millis, bytes)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --no-default-features --lib -p ymux paste_images::`
Expected: PASS (4 tests).
Then: `cargo check --no-default-features --lib --tests -p ymux` → clean (Linux gate).
Then: `cargo fmt --all` and confirm `cargo fmt --all --check` is clean.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/paste_images.rs src-tauri/src/lib.rs
git commit -m "feat(paste): pure paste-images module — save + timestamp-based prune"
```

---

## Task 2: `paste_image_retention_hours` config field

**Files:**
- Modify: `src-tauri/src/config/model.rs` (`Config` struct + `Config::default()` + every test `Config { .. }` literal + a default test)
- Modify: `src/types.ts` (`Config` interface)

**Interfaces:**
- Produces: `Config.paste_image_retention_hours: u32` (default 24), read later by the `save_paste_image` command.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src-tauri/src/config/model.rs`, mirroring `persist_scrollback_defaults_true_when_absent`:

```rust
#[test]
fn paste_image_retention_hours_defaults_to_24_when_absent() {
    let toml_str = "version = 5\nactive_workspace = 1\n";
    let parsed: Config = toml::from_str(toml_str).expect("deserialize");
    assert_eq!(parsed.paste_image_retention_hours, 24);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --no-default-features --lib -p ymux config::model::tests::paste_image_retention_hours`
Expected: FAIL to compile — no field `paste_image_retention_hours` on `Config`.

- [ ] **Step 3: Add the field (mirror `persist_scrollback`)**

In `src-tauri/src/config/model.rs`, add to the `Config` struct (right after `persist_scrollback`):

```rust
    #[serde(default = "default_paste_image_retention_hours")]
    pub paste_image_retention_hours: u32,
```

Add the default fn next to `default_persist_scrollback`:

```rust
fn default_paste_image_retention_hours() -> u32 {
    24
}
```

Set `paste_image_retention_hours: 24` in `Config::default()` and in **every** explicit `Config { .. }` literal in the `#[cfg(test)]` module (the compiler flags each missing one — grep the file for `persist_scrollback:` occurrences and add the new field beside each).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --no-default-features --lib -p ymux config::model`
Expected: PASS (all config tests, including the new one).

- [ ] **Step 5: Mirror the field in the TS `Config` type**

In `src/types.ts`, add to the `Config` interface next to `persist_scrollback` (non-optional `number`, matching the always-serialized Rust field):

```ts
  paste_image_retention_hours: number;
```

Run: `npx tsc --noEmit` → clean.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/config/model.rs src/types.ts
git commit -m "feat(paste): add paste_image_retention_hours config field (default 24h)"
```

---

## Task 3: `save_paste_image` Tauri command

**Files:**
- Modify: `src-tauri/src/commands.rs` (new command)
- Modify: `src-tauri/src/main.rs` (register in `generate_handler!`)

**Interfaces:**
- Consumes: `crate::paste_images::save` (Task 1), `Config.paste_image_retention_hours` (Task 2).
- Produces (TS-visible command): `save_paste_image(bytes: number[]) -> string` (the saved absolute path).

- [ ] **Step 1: Write the command**

Add to `src-tauri/src/commands.rs` (after `save_scrollback`, following the same thin-wrapper style; `State`/`AppState`/`YmuxError` are already imported):

```rust
/// Save a pasted clipboard image (raw PNG bytes) to the paste-images dir,
/// pruning images older than the configured retention window first, and
/// return the absolute path so the frontend can type it into the PTY.
#[tauri::command]
pub fn save_paste_image(state: State<AppState>, bytes: Vec<u8>) -> YmuxResult<String> {
    let hours = state.config.snapshot().paste_image_retention_hours;
    let retention = std::time::Duration::from_secs(u64::from(hours) * 3600);
    let path = crate::paste_images::save(&bytes, retention).map_err(YmuxError::Io)?;
    Ok(path.to_string_lossy().into_owned())
}
```

- [ ] **Step 2: Register the command**

In `src-tauri/src/main.rs`, add to the `tauri::generate_handler![ ... ]` list after the scrollback commands:

```rust
            ymux_lib::commands::save_paste_image,
```

- [ ] **Step 3: Verify it compiles (both feature modes)**

Run: `cargo check --no-default-features --lib --tests -p ymux` → clean (Linux gate; `commands.rs` is gated out here, but nothing else must break).
Run: `cargo check -p ymux` → **must compile** (this is the default desktop build that actually includes `commands.rs`; a struct/command mismatch fails here).
Run: `cargo fmt --all` then `cargo fmt --all --check` → clean.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/main.rs
git commit -m "feat(paste): save_paste_image command reading retention from config"
```

---

## Task 4: `savePasteImage` bridge wrapper

**Files:**
- Modify: `src/ipc/bridge.ts`

**Interfaces:**
- Produces: `api.savePasteImage(bytes: number[]) -> Promise<string>`.

- [ ] **Step 1: Add the wrapper**

In `src/ipc/bridge.ts`, add to the `api` object mirroring the existing `saveScrollback` wrapper (use the same `call(...)` helper):

```ts
  /// Save a pasted clipboard image (PNG bytes) to a temp file; returns its path.
  savePasteImage: (bytes: number[]): Promise<string> =>
    call("save_paste_image", { bytes }),
```

- [ ] **Step 2: Verify + commit**

Run: `npx tsc --noEmit` → clean.

```bash
git add src/ipc/bridge.ts
git commit -m "feat(paste): savePasteImage bridge wrapper"
```

---

## Task 5: Rework `TerminalPane.pasteClipboard()` — image-first, text preserved

**Files:**
- Modify: `src/terminal/TerminalPane.ts` (`pasteClipboard()`, ~line 509)

**Interfaces:**
- Consumes: `api.savePasteImage` (Task 4), the existing `api.writePane` + `ENCODER` + `this.spawned` + `this.id`.

- [ ] **Step 1: Replace `pasteClipboard()`**

Current code (verified) at `src/terminal/TerminalPane.ts:509`:

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

Replace it with the image-first version (the text branch below is byte-for-byte the existing behaviour):

```ts
  private async pasteClipboard(): Promise<void> {
    // Image first: if the clipboard holds a PNG, save it to a temp file and
    // paste the file's path (so an in-pane CLI can read the image).
    try {
      const items = await navigator.clipboard.read();
      for (const item of items) {
        if (item.types.includes("image/png")) {
          const blob = await item.getType("image/png");
          const bytes = new Uint8Array(await blob.arrayBuffer());
          const path = await api.savePasteImage(Array.from(bytes));
          if (path && this.spawned) {
            // Path only — no trailing newline; the user presses Enter.
            void api.writePane(this.id, ENCODER.encode(path));
          }
          return;
        }
      }
    } catch {
      // clipboard.read() unsupported/denied, or save failed — fall through to
      // the text path below.
    }
    // Existing text-paste behaviour, unchanged.
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

- [ ] **Step 2: Verify types + existing tests**

Run: `npx tsc --noEmit` → clean.
Run: `pnpm exec vitest run` → still green (this change adds no unit tests; the clipboard path is manual-smoke only — noted in Step 3).

- [ ] **Step 3: Manual smoke test (Windows, `pnpm tauri dev`)**

This is the only place the end-to-end clipboard path can be exercised. Confirm:
1. Copy a screenshot to the clipboard (e.g. `Win+Shift+S`), focus a terminal pane, press **Ctrl+V** → the pane receives a file **path** (e.g. `C:\Users\...\ymux\paste-images\clip-<ms>.png`), NOT raw text, and NO Enter is sent.
2. The file exists at that path and is a valid PNG (open it).
3. Copy plain **text**, Ctrl+V in a pane → text is pasted as before (image path logic did not interfere).
4. Confirm old files in `paste-images/` are pruned after the retention window (or lower `paste_image_retention_hours` in `config.toml` to verify pruning quickly).

If `pnpm tauri dev` can't run in the implementer's environment, report the smoke as deferred to a human — the automated `cargo`/`tsc`/`vitest` gates above are the CI-level guarantee.

- [ ] **Step 4: Commit**

```bash
git add src/terminal/TerminalPane.ts
git commit -m "feat(paste): Ctrl+V pastes a clipboard image as a temp-file path"
```

---

## Notes

- **Version bump** to v0.8.20 (6 sync points per CLAUDE.md rule #5) and release are intentionally **out of this plan** — do them at release time, possibly batching with other v0.8.20 work.
- **Millisecond filename collision:** two pastes within the same millisecond would collide on `clip-<millis>.png` (the second overwrites the first). This is effectively impossible for manual Ctrl+V (pastes are seconds apart) and is left unhandled per YAGNI.
