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
        Ok(Self {
            path,
            inner: Mutex::new(Inner {
                config,
                dirty: false,
            }),
        })
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
        let (to_write, was_dirty) = {
            let mut guard = self.inner.lock();
            if !guard.dirty {
                return Ok(false);
            }
            let cloned = guard.config.clone();
            guard.dirty = false;
            (cloned, true)
        };
        if was_dirty {
            write_atomic(&self.path, &to_write)?;
        }
        Ok(was_dirty)
    }

    /// Force an immediate write regardless of dirty flag. Used on
    /// `close-requested`.
    pub fn flush(&self) -> YmuxResult<()> {
        let cloned = {
            let mut guard = self.inner.lock();
            guard.dirty = false;
            guard.config.clone()
        };
        write_atomic(&self.path, &cloned)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Serialize `config` to TOML and atomically rename into place so that a
/// crash mid-write cannot corrupt the file.
fn write_atomic(path: &Path, config: &Config) -> YmuxResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(config)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, path)?;
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
