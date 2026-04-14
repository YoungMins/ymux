use serde::Serialize;
use thiserror::Error;

/// Result alias used across the ymux crate.
pub type YmuxResult<T> = Result<T, YmuxError>;

/// Top-level error type. Implements `Serialize` so it can be returned directly
/// from `#[tauri::command]` handlers and surfaced to the frontend as a tagged
/// JSON object.
#[derive(Debug, Error)]
pub enum YmuxError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml deserialize: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unknown pane id: {0}")]
    UnknownPane(uuid::Uuid),

    #[error("unknown shell profile: {0}")]
    UnknownShell(String),

    #[error("no shells detected on this system")]
    NoShells,

    #[error("pty: {0}")]
    Pty(String),

    #[error("config: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for YmuxError {
    fn from(value: anyhow::Error) -> Self {
        YmuxError::Other(value.to_string())
    }
}

impl Serialize for YmuxError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Flatten to a simple string payload: the frontend doesn't need the
        // whole enum discriminant, and this keeps the wire format stable.
        serializer.serialize_str(&self.to_string())
    }
}
