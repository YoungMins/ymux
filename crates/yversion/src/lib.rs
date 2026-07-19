//! Shared release version for the ymux tool family.
//!
//! Single source of truth so the footer of every TUI (ymon / ydir / ycode /
//! ygit) stays in lockstep with the ymux app version. Keep this constant in
//! sync with `src-tauri/Cargo.toml :: version` — the release checklist in
//! `CLAUDE.md` lists this file alongside the others to bump.

pub const VERSION: &str = "0.8.22";
