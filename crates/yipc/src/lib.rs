//! # yipc — Inter-process communication for ymux panes
//!
//! Provides a lightweight, blocking IPC mechanism that allows TUI tools running
//! inside PTY panes to communicate with the ymux host process (and through it,
//! with each other).
//!
//! ## Protocol
//!
//! Messages are newline-delimited JSON (`\n` terminated). Each line decodes
//! into an [`IpcMessage`].
//!
//! ## Transport
//!
//! - **Unix**: Unix domain socket at a path like `/tmp/ymux-{session-uuid}.sock`
//! - **Windows**: TCP on localhost (`127.0.0.1:<port>`) as a portable fallback
//!
//! The address is communicated to child processes via the `YMUX_IPC` environment
//! variable.

mod client;
mod protocol;
mod server;

pub use client::IpcClient;
pub use protocol::{
    open_file_event, open_file_path, CommandDef, IpcMessage, AGENT_HOOK_KIND, OPEN_FILE_KIND,
};
pub use server::{IpcServer, MessageHandler, WRITE_TIMEOUT};

/// Errors produced by the IPC layer.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("environment variable YMUX_IPC not set")]
    EnvNotSet,
    #[error("connection closed")]
    ConnectionClosed,
    #[error("server stopped")]
    ServerStopped,
}

pub type IpcResult<T> = Result<T, IpcError>;
