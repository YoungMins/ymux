# ymux v0.8.17

The whole y* family now tells you which version is running, without alt-tabbing back to the Settings panel.

## Features

### 🏷️ Version badge in every TUI footer

`ymon`, `ydir`, `ycode`, and `ygit` each render the current ymux release version (`v0.8.17`) right-aligned in their existing status / footer bar. The hint text on the left keeps its position — the version just sits flush against the right edge in the teal accent so it's findable without crowding the shortcuts.

### ⚙️ Version row in yMux Settings → General → About

The Settings panel's About section grows a "Version" row showing the running app version (`v0.8.17`) in monospace teal. The value comes straight from `tauri.conf.json` via `@tauri-apps/api/app::getVersion()`, so it stays in sync with whichever MSI you installed — no manual wiring on the frontend side. i18n: `Version` / `버전` / `バージョン`.

## Under the hood

- New `crates/yversion` workspace member holds a single `pub const VERSION` that all four TUI tools depend on. The release checklist in `CLAUDE.md` now lists `crates/yversion/src/lib.rs` alongside the other version files so they stay locked together.
- ymon / ydir / ygit footers gained a horizontal split (`Constraint::Min(0) + Constraint::Length(version_width)`) so the version sits in its own area without nudging the hint text. ycode's status line was already left+right with computed padding, so the version just appends to the right-side format string.
- ygit needed an explicit `Alignment` import — its `ui.rs` doesn't pull in `ratatui::prelude::*` the way the other three do.

## Known issue

The markdown scroll-ghost reported in v0.8.16 still reproduces in some setups. The v0.8.16 empty-syntect-span filter helped but didn't fully resolve it — under closer testing, the ghosts still appear on certain files. Investigation parked for now to ship this version-display work; will resume in a follow-up release.

## Compatibility

Drop-in. No config or behavior changes outside the new footer character + Settings row.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.16...v0.8.17
