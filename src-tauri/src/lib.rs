//! ymux library crate.
//!
//! All non-`main.rs` code lives here so that unit tests, `cargo check`, and
//! `cargo clippy` work even on hosts where the full Tauri runtime toolchain
//! (WebView2, bundler, etc.) is not available.

pub mod config;
pub mod error;
pub mod pty;
pub mod shell;

// `commands` exists only when the desktop feature is enabled, because it
// references the Tauri runtime types (`State`, `AppHandle`, `Emitter`, ...).
// Living inside the lib crate (rather than as a sibling module of `main.rs`)
// means the `crate::config` / `crate::pty` paths inside the file resolve
// correctly without having to reach across crate boundaries.
#[cfg(feature = "desktop")]
pub mod commands;

// Update checker. Feature-gated the same way as `commands` because it emits
// Tauri events and pulls `reqwest` — both only relevant in the desktop build.
#[cfg(feature = "desktop")]
pub mod updater;

// Native browser child webview management. Desktop-only because it uses
// WebviewWindowBuilder and Manager traits from Tauri.
#[cfg(feature = "desktop")]
pub mod webview;

// Embedded browser child webview panes via Window::add_child (Tauri unstable).
#[cfg(feature = "desktop")]
pub mod embedded_browser;

// System resource monitor (CPU, RAM, disk, network, GPU). Desktop-only
// because it emits Tauri events.
#[cfg(feature = "desktop")]
pub mod sysmonitor;

// Inter-pane IPC server. Desktop-only because it emits Tauri events and
// requires the yipc crate.
#[cfg(feature = "desktop")]
pub mod ipc_server;

// Settings panel backend — ytheme load/save + open-with-default-app for
// the Config Files section. Desktop-only because the commands plug into
// the Tauri invoke handler.
#[cfg(feature = "desktop")]
pub mod settings;

pub use error::{YmuxError, YmuxResult};
