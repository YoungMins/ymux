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

    #[error("git: {0}")]
    Git(String),

    #[error("config: {0}")]
    Config(String),

    // ---------------------------------------------------------------------
    // Variants the filesystem / text-file / git surface returns. These exist
    // so the frontend can *branch* on a failure rather than substring-match
    // an English sentence: `kind()` below is their stable machine name.
    // ---------------------------------------------------------------------
    /// The path does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The OS refused the operation (ACL, read-only volume, locked file).
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The destination of a create / rename / copy is already taken and the
    /// caller did not ask for an overwrite.
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// The file is not valid UTF-8. Deliberately *not* lossy-decoded: a
    /// lossy read followed by a save destroys the file (spec §1.2).
    #[error("not valid UTF-8: {0}")]
    NotUtf8(String),

    /// Over `textfile::MAX_EDIT_BYTES`. Refusing loudly beats hanging.
    #[error("too large: {0}")]
    TooLarge(String),

    /// A stamped write lost the race: the file changed on disk since the
    /// caller read it. The editor pane turns this into Reload / Overwrite /
    /// Save-as-copy rather than silently clobbering an agent's edit.
    #[error("changed on disk since it was read: {0}")]
    Conflict(String),

    /// The directory is not inside a git work tree.
    #[error("not a git repository: {0}")]
    NotARepo(String),

    /// The caller is not ymux's own document. See [`crate::fspath::guard_local`]
    /// — this is the *only* gate on the fs/git surface, because ymux's own
    /// commands sit outside Tauri's capability system (spec §1.5).
    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("{0}")]
    Other(String),
}

impl YmuxError {
    /// Stable, machine-readable name of the variant, for the frontend to
    /// branch on. These strings are a wire contract: renaming one is a
    /// breaking change even though nothing in Rust will complain.
    pub fn kind(&self) -> &'static str {
        match self {
            YmuxError::Io(_) => "io",
            YmuxError::TomlDe(_) | YmuxError::TomlSer(_) => "toml",
            YmuxError::Json(_) => "json",
            YmuxError::UnknownPane(_) => "unknown_pane",
            YmuxError::UnknownShell(_) => "unknown_shell",
            YmuxError::NoShells => "no_shells",
            YmuxError::Pty(_) => "pty",
            YmuxError::Git(_) => "git",
            YmuxError::Config(_) => "config",
            YmuxError::NotFound(_) => "not_found",
            YmuxError::PermissionDenied(_) => "permission_denied",
            YmuxError::AlreadyExists(_) => "already_exists",
            YmuxError::NotUtf8(_) => "not_utf8",
            YmuxError::TooLarge(_) => "too_large",
            YmuxError::Conflict(_) => "conflict",
            YmuxError::NotARepo(_) => "not_a_repo",
            YmuxError::Forbidden(_) => "forbidden",
            YmuxError::Other(_) => "other",
        }
    }

    /// Classify an [`std::io::Error`] into the variants the frontend can act
    /// on, keeping `path` in the message so the banner can name the file.
    ///
    /// Used instead of the blanket `From<io::Error>` everywhere the fs
    /// surface touches a specific path — `From` stays for the callers that
    /// predate this and have nothing useful to name.
    pub fn from_io(err: &std::io::Error, path: &str) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => YmuxError::NotFound(path.to_string()),
            std::io::ErrorKind::PermissionDenied => YmuxError::PermissionDenied(path.to_string()),
            std::io::ErrorKind::AlreadyExists => YmuxError::AlreadyExists(path.to_string()),
            _ => YmuxError::Other(format!("{path}: {err}")),
        }
    }
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
        // `{ kind, message }`, not a bare string. `message` carries exactly
        // what the old `serialize_str` sent, and `src/ipc/bridge.ts`'s
        // `describeError` reads `obj.message` before anything else, so every
        // error string the UI already shows is byte-identical. `kind` is
        // additive: the new fs/git callers branch on it (spec §1.6) and
        // nothing else has to care.
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("YmuxError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape the frontend depends on. `message` must stay the plain
    /// `Display` string so `describeError` keeps rendering what it always did.
    #[test]
    fn serializes_as_kind_and_message() {
        let json = serde_json::to_value(YmuxError::NotFound("C:\\x".into())).unwrap();
        assert_eq!(json["kind"], "not_found");
        assert_eq!(json["message"], "not found: C:\\x");
    }

    /// The pre-existing variants must not have their message text changed by
    /// the move to a struct payload — the UI shows these strings today.
    #[test]
    fn existing_variant_messages_are_unchanged() {
        let json = serde_json::to_value(YmuxError::Pty("spawn failed".into())).unwrap();
        assert_eq!(json["kind"], "pty");
        assert_eq!(json["message"], "pty: spawn failed");

        let json = serde_json::to_value(YmuxError::Other("boom".into())).unwrap();
        assert_eq!(json["message"], "boom");
    }

    /// `kind()` is a wire contract; this pins every name so a rename is a
    /// deliberate act with a failing test, not a silent frontend break.
    #[test]
    fn kind_names_are_stable() {
        assert_eq!(YmuxError::NoShells.kind(), "no_shells");
        assert_eq!(YmuxError::NotFound(String::new()).kind(), "not_found");
        assert_eq!(
            YmuxError::PermissionDenied(String::new()).kind(),
            "permission_denied"
        );
        assert_eq!(
            YmuxError::AlreadyExists(String::new()).kind(),
            "already_exists"
        );
        assert_eq!(YmuxError::NotUtf8(String::new()).kind(), "not_utf8");
        assert_eq!(YmuxError::TooLarge(String::new()).kind(), "too_large");
        assert_eq!(YmuxError::Conflict(String::new()).kind(), "conflict");
        assert_eq!(YmuxError::NotARepo(String::new()).kind(), "not_a_repo");
        assert_eq!(YmuxError::Forbidden(String::new()).kind(), "forbidden");
    }

    #[test]
    fn from_io_classifies_the_kinds_the_ui_branches_on() {
        let nf = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        assert_eq!(YmuxError::from_io(&nf, "/a/b").kind(), "not_found");

        let pd = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        assert_eq!(YmuxError::from_io(&pd, "/a/b").kind(), "permission_denied");

        let ae = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "nope");
        assert_eq!(YmuxError::from_io(&ae, "/a/b").kind(), "already_exists");

        // Anything else keeps the path but does not pretend to a category.
        let other = std::io::Error::other("weird");
        let e = YmuxError::from_io(&other, "/a/b");
        assert_eq!(e.kind(), "other");
        assert!(e.to_string().contains("/a/b"));
    }
}
