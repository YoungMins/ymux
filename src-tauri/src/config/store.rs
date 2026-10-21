//! On-disk persistence for [`Config`].
//!
//! The config is a TOML file at `%APPDATA%/ymux/config.toml` on Windows
//! (`$XDG_CONFIG_HOME/ymux/config.toml` elsewhere, used for Linux development).
//!
//! [`ConfigStore`] owns an in-memory copy protected by a [`parking_lot::Mutex`]
//! and debounces writes: callers mutate through [`ConfigStore::update`] which
//! marks the store dirty, and [`ConfigStore::flush_if_dirty`] persists.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use super::model::Config;
use crate::error::YmuxResult;

/// Relative path below `dirs::config_dir()` (or `%APPDATA%`).
const SUBDIR: &str = "ymux";
const FILE: &str = "config.toml";

/// Return the resolved config file path. Falls back to `./ymux-config.toml`
/// if no platform config directory is available.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join(SUBDIR).join(FILE))
        .unwrap_or_else(|| PathBuf::from("./ymux-config.toml"))
}

/// Thread-safe, dirty-tracking wrapper around a [`Config`].
pub struct ConfigStore {
    path: PathBuf,
    inner: Mutex<Inner>,
    /// Held from the snapshot through the rename, so concurrent flushes land
    /// in order and the last one on disk is the latest state.
    write: Mutex<()>,
}

struct Inner {
    config: Config,
    dirty: bool,
}

impl ConfigStore {
    /// Load the config from `path`, or produce a default one if the file does
    /// not exist. Parse errors are hard errors; it is better to stop than to
    /// silently clobber a user's config.
    pub fn load(path: impl Into<PathBuf>) -> YmuxResult<Self> {
        let path: PathBuf = path.into();
        let config = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str::<Config>(&text)?
        } else {
            Config::default()
        };
        Ok(Self::from_config(path, config))
    }

    /// A store for an already-parsed (or default) config, migrated.
    fn from_config(path: PathBuf, mut config: Config) -> Self {
        // Bring the in-memory config up to the current schema version so the
        // rest of the app can assume current semantics. If `migrate` changes
        // anything, mark dirty so the next flush writes it back.
        let pre_migrate_version = config.version;
        config.migrate();
        let dirty = config.version != pre_migrate_version;
        Self {
            path,
            inner: Mutex::new(Inner { config, dirty }),
            write: Mutex::new(()),
        }
    }

    /// Startup load that never refuses to start and never strands saves in a
    /// throwaway file without cause (see [`LoadOutcome`]):
    ///
    /// - no file: a default config at `path` (its directory is created; if
    ///   that fails, the fallback below);
    /// - a file that is not valid TOML / not a valid config: moved aside to
    ///   `<name>.broken-<now_unix>` and replaced by a default config at
    ///   `path`, so the next save writes a good file where the user's was;
    /// - a file that cannot be read, or a broken one that cannot be moved
    ///   aside: a default config at `fallback`, because saving at `path`
    ///   would overwrite the user's file.
    pub fn load_or_recover(path: &Path, fallback: &Path, now_unix: u64) -> (Self, LoadOutcome) {
        let default_at = |p: &Path| Self::from_config(p.to_path_buf(), Config::default());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let dir_ok = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    Some(dir) => std::fs::create_dir_all(dir).is_ok() && dir.is_dir(),
                    None => true,
                };
                return if dir_ok {
                    (default_at(path), LoadOutcome::Fresh)
                } else {
                    (
                        default_at(fallback),
                        LoadOutcome::Fallback {
                            error: format!("config directory of {} is unusable", path.display()),
                        },
                    )
                };
            }
            Err(e) => {
                return (
                    default_at(fallback),
                    LoadOutcome::Fallback {
                        error: format!("cannot read {}: {e}", path.display()),
                    },
                )
            }
        };
        let parsed = String::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<Config>(&text).map_err(|e| e.to_string()));
        let error = match parsed {
            Ok(config) => {
                return (
                    Self::from_config(path.to_path_buf(), config),
                    LoadOutcome::Loaded,
                )
            }
            Err(e) => e,
        };
        let broken = broken_path(path, now_unix);
        match std::fs::rename(path, &broken) {
            Ok(()) => {
                crate::durable::sync_parent(path);
                (default_at(path), LoadOutcome::Recovered { broken, error })
            }
            Err(e) => (
                default_at(fallback),
                LoadOutcome::Fallback {
                    error: format!(
                        "{error}; and moving it aside to {} failed: {e}",
                        broken.display()
                    ),
                },
            ),
        }
    }

    /// Convenience wrapper that loads from the default platform path.
    pub fn load_default() -> YmuxResult<Self> {
        Self::load(config_path())
    }

    /// Take a snapshot of the current config. Cheap-ish — clones the whole
    /// tree, but the tree is small.
    pub fn snapshot(&self) -> Config {
        self.inner.lock().config.clone()
    }

    /// Apply a mutation with the in-memory config under lock and mark dirty.
    pub fn update<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Config) -> R,
    {
        let mut guard = self.inner.lock();
        let r = f(&mut guard.config);
        guard.dirty = true;
        r
    }

    /// Replace the config wholesale (e.g. when the frontend ships the full
    /// state back). Marks dirty.
    pub fn replace(&self, new_config: Config) {
        let mut guard = self.inner.lock();
        guard.config = new_config;
        guard.dirty = true;
    }

    /// Persist to disk if anything has changed since the last flush.
    pub fn flush_if_dirty(&self) -> YmuxResult<bool> {
        let _write = self.write.lock();
        let to_write = {
            let mut guard = self.inner.lock();
            if !guard.dirty {
                return Ok(false);
            }
            guard.dirty = false;
            guard.config.clone()
        };
        if let Err(e) = write_atomic(&self.path, &to_write) {
            self.mark_dirty();
            return Err(e);
        }
        Ok(true)
    }

    /// A write failed: keep the state marked unsaved so the next flush
    /// retries instead of believing it is on disk.
    fn mark_dirty(&self) {
        self.inner.lock().dirty = true;
    }

    /// Force an immediate write regardless of dirty flag. Used on
    /// `close-requested`.
    pub fn flush(&self) -> YmuxResult<()> {
        let _write = self.write.lock();
        let cloned = {
            let mut guard = self.inner.lock();
            guard.dirty = false;
            guard.config.clone()
        };
        let written = write_atomic(&self.path, &cloned);
        if written.is_err() {
            self.mark_dirty();
        }
        written
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// How [`ConfigStore::load_or_recover`] came by its config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// Parsed from the file at the real path.
    Loaded,
    /// There was no file: a default config, saved to the real path.
    Fresh,
    /// The file did not parse (`error`) and was moved to `broken`; a default
    /// config is saved to the real path.
    Recovered { broken: PathBuf, error: String },
    /// The real path is unusable: a default config saved to the fallback.
    Fallback { error: String },
}

impl LoadOutcome {
    /// Whether the layout really came from the user's file (only then may it
    /// prune state keyed by pane ids).
    pub fn from_disk(&self) -> bool {
        matches!(self, LoadOutcome::Loaded)
    }
}

/// `config.toml` → `config.toml.broken-<now_unix>`.
fn broken_path(path: &Path, now_unix: u64) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| FILE.into());
    name.push(format!(".broken-{now_unix}"));
    path.with_file_name(name)
}

/// Serialize `config` to TOML and replace the file durably (temp file,
/// `sync_all`, rename; see [`crate::durable`]) so that a crash mid-write
/// cannot corrupt it and a failed write leaves no temp file behind.
fn write_atomic(path: &Path, config: &Config) -> YmuxResult<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(config)?;
    crate::durable::write_atomic(path, text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{PaneSpec, ShellProfile, Workspace};

    #[test]
    fn load_missing_file_yields_default() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        let store = ConfigStore::load(&path).expect("load");
        let snap = store.snapshot();
        assert_eq!(snap.active_workspace, 1);
        assert_eq!(snap.workspaces.len(), 1);
    }

    #[test]
    fn update_and_flush_persists_changes() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        let store = ConfigStore::load(&path).expect("load");
        store.update(|cfg| {
            cfg.shells.push(ShellProfile {
                name: "cmd".into(),
                executable: "C:\\Windows\\System32\\cmd.exe".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            });
        });
        assert!(store.flush_if_dirty().expect("flush"));
        // Second flush is a no-op because the dirty flag reset.
        assert!(!store.flush_if_dirty().expect("flush"));

        let reloaded = ConfigStore::load(&path).expect("reload");
        let snap = reloaded.snapshot();
        assert_eq!(snap.shells.len(), 1);
        assert_eq!(snap.shells[0].name, "cmd");
    }

    #[test]
    fn replace_swaps_whole_config() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        let store = ConfigStore::load(&path).expect("load");
        let mut new_cfg = Config::default();
        new_cfg.workspaces.clear();
        new_cfg.workspaces.push(Workspace {
            id: 3,
            name: "three".into(),
            account: String::new(),
            root: super::super::model::LayoutNode::Pane(PaneSpec::new_default()),
        });
        new_cfg.active_workspace = 3;
        store.replace(new_cfg);
        store.flush().expect("flush");

        let reloaded = ConfigStore::load(&path).expect("reload");
        let snap = reloaded.snapshot();
        assert_eq!(snap.active_workspace, 3);
        assert_eq!(snap.workspaces[0].id, 3);
        assert_eq!(snap.workspaces[0].name, "three");
    }

    #[test]
    fn concurrent_flushes_end_with_latest_state() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        let store = std::sync::Arc::new(ConfigStore::load(&path).expect("load"));
        let handles: Vec<_> = (0..8u16)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for j in 0..10u16 {
                        store.update(|c| c.agent_hook_port = i * 100 + j + 1);
                        store.flush().expect("flush");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread");
        }
        store.update(|c| c.agent_hook_port = 4242);
        store.flush_if_dirty().expect("flush");
        let reloaded = ConfigStore::load(&path).expect("reload");
        assert_eq!(reloaded.snapshot().agent_hook_port, 4242);
    }

    #[test]
    fn failed_flush_keeps_the_store_dirty_and_retries() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        // A directory where the file should be: every write fails.
        std::fs::create_dir(&path).expect("mkdir in the file's place");
        let store = ConfigStore::load(dir.join("missing.toml")).expect("load");
        let store = ConfigStore {
            path: path.clone(),
            ..store
        };
        store.update(|c| c.agent_hook_port = 777);
        assert!(store.flush_if_dirty().is_err());
        assert!(store.inner.lock().dirty, "a failed write must stay dirty");
        assert!(store.flush().is_err());
        assert!(
            store.inner.lock().dirty,
            "a failed forced flush must stay dirty"
        );
        // No temp file leaked next to the target.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        // Once the path is writable the pending change is written.
        std::fs::remove_dir(&path).unwrap();
        assert!(store.flush_if_dirty().expect("retry"));
        let reloaded = ConfigStore::load(&path).expect("reload");
        assert_eq!(reloaded.snapshot().agent_hook_port, 777);
    }

    #[test]
    fn unwritable_dir_keeps_the_store_dirty() {
        let dir = tempdir();
        // A regular file where the config directory should be.
        let blocker = dir.join("ymux");
        std::fs::write(&blocker, b"").unwrap();
        let store = ConfigStore::load(blocker.join("config.toml")).expect("load");
        store.update(|c| c.agent_hook_port = 1);
        assert!(store.flush_if_dirty().is_err());
        assert!(store.inner.lock().dirty);
    }

    #[test]
    fn recover_missing_file_is_a_fresh_default_at_the_real_path() {
        let dir = tempdir();
        let path = dir.join("sub").join("config.toml");
        let fallback = dir.join("fallback.toml");
        let (store, outcome) = ConfigStore::load_or_recover(&path, &fallback, 1);
        assert_eq!(outcome, LoadOutcome::Fresh);
        assert!(!outcome.from_disk());
        assert_eq!(store.path(), path.as_path());
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn recover_valid_file_loads_it() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        let store = ConfigStore::load(&path).unwrap();
        store.update(|c| c.agent_hook_port = 4321);
        store.flush().unwrap();
        let (store, outcome) = ConfigStore::load_or_recover(&path, &dir.join("fb.toml"), 1);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert!(outcome.from_disk());
        assert_eq!(store.snapshot().agent_hook_port, 4321);
    }

    #[test]
    fn recover_moves_a_corrupt_file_aside_and_saves_at_the_real_path() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(&path, b"this is [not toml").unwrap();
        let (store, outcome) = ConfigStore::load_or_recover(&path, &dir.join("fb.toml"), 1700);
        let broken = dir.join("config.toml.broken-1700");
        match &outcome {
            LoadOutcome::Recovered { broken: b, .. } => assert_eq!(b, &broken),
            other => panic!("unexpected outcome {other:?}"),
        }
        assert!(!outcome.from_disk());
        assert_eq!(std::fs::read(&broken).unwrap(), b"this is [not toml");
        assert_eq!(store.path(), path.as_path());
        store.flush().expect("save at the real path");
        assert!(ConfigStore::load(&path).is_ok());
    }

    #[test]
    fn recover_non_utf8_file_counts_as_corrupt() {
        let dir = tempdir();
        let path = dir.join("config.toml");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let (_, outcome) = ConfigStore::load_or_recover(&path, &dir.join("fb.toml"), 2);
        assert!(
            matches!(outcome, LoadOutcome::Recovered { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn recover_unusable_dir_falls_back() {
        let dir = tempdir();
        let blocker = dir.join("ymux");
        std::fs::write(&blocker, b"").unwrap();
        let path = blocker.join("config.toml");
        let fallback = dir.join("fb.toml");
        let (store, outcome) = ConfigStore::load_or_recover(&path, &fallback, 3);
        assert!(
            matches!(outcome, LoadOutcome::Fallback { .. }),
            "{outcome:?}"
        );
        assert_eq!(store.path(), fallback.as_path());
    }

    #[test]
    fn recover_unreadable_file_falls_back_without_touching_it() {
        let dir = tempdir();
        // Reading a directory fails with something other than NotFound.
        let path = dir.join("config.toml");
        std::fs::create_dir(&path).unwrap();
        let fallback = dir.join("fb.toml");
        let (store, outcome) = ConfigStore::load_or_recover(&path, &fallback, 4);
        assert!(
            matches!(outcome, LoadOutcome::Fallback { .. }),
            "{outcome:?}"
        );
        assert_eq!(store.path(), fallback.as_path());
        assert!(path.is_dir());
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }
}
