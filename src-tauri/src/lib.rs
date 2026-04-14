//! ymux library crate.
//!
//! All non-`main.rs` code lives here so that unit tests, `cargo check`, and
//! `cargo clippy` work even on hosts where the full Tauri runtime toolchain
//! (WebView2, bundler, etc.) is not available.

pub mod config;
pub mod error;
pub mod pty;
pub mod shell;

pub use error::{YmuxError, YmuxResult};
