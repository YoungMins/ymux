//! Owner of all [`PtySession`]s for the running app.
//!
//! The manager keeps a registry keyed by pane [`Uuid`] and centralises the
//! reader → frontend event channel. Each spawned pane pushes its bytes into a
//! single `mpsc` channel; the caller (the Tauri layer) drains it on a
//! dedicated thread and forwards events to the webview.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use parking_lot::Mutex;
use portable_pty::PtySize;
use uuid::Uuid;

use crate::config::model::{PaneSpec, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::pty::session::{CwdMap, PaneEvent, PtySession};

/// Build a `PATH` with `dir` — the directory holding ymux's sidecar tools —
/// at the front, or `None` if it is already there.
///
/// Windows gets this from the MSI, which writes the install directory into the
/// system `PATH`. macOS has no equivalent: a `.app` bundle cannot register
/// anything, and the tools sit inside it at `Contents/MacOS`, invisible to the
/// shell. So the app puts its own directory on the `PATH` of every PTY it
/// spawns — it knows where its sidecars are, and asking the user to paste an
/// `export` into their dotfiles for something the app already knows is a poor
/// trade.
///
/// Prepended rather than appended so the bundled tools win over an older copy
/// left behind by a previous install.
pub fn path_with_sidecar_dir(dir: &Path, current: Option<&OsStr>) -> Option<OsString> {
    let existing: Vec<PathBuf> = current
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    if existing.iter().any(|p| p == dir) {
        return None;
    }
    let joined = std::iter::once(dir.to_path_buf()).chain(existing);
    std::env::join_paths(joined).ok()
}

/// `PATH` entry for the running executable's own directory, ready to hand to
/// [`PtyManager::set_extra_env`]. `None` when the path can't be resolved or
/// already contains it.
pub fn sidecar_path_entry() -> Option<(String, String)> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let next = path_with_sidecar_dir(dir, std::env::var_os("PATH").as_deref())?;
    Some(("PATH".to_string(), next.to_string_lossy().into_owned()))
}

/// Metadata returned to the frontend after a successful spawn.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpawnedPane {
    pub id: Uuid,
    pub shell: String,
}

pub struct PtyManager {
    sessions: Mutex<HashMap<Uuid, PtySession>>,
    tx: Sender<PaneEvent>,
    // Held so it doesn't drop; consumers take it with `take_event_receiver`.
    rx: Mutex<Option<Receiver<PaneEvent>>>,
    // Shared `pane id → latest cwd` map. Reader threads push updates into it
    // as they parse OSC 7 sequences, and `save_config` reads from it to
    // patch the persisted layout with live working directories.
    cwds: CwdMap,
    // Extra environment variables injected into every spawned PTY process
    // (e.g. `YMUX_IPC`). Set once at startup, read on every spawn.
    extra_env: Mutex<Vec<(String, String)>>,
}

impl Default for PtyManager {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            sessions: Mutex::new(HashMap::new()),
            tx,
            rx: Mutex::new(Some(rx)),
            cwds: Arc::new(Mutex::new(HashMap::new())),
            extra_env: Mutex::new(Vec::new()),
        }
    }
}

impl PtyManager {
    /// Take ownership of the event receiver. The caller is expected to park a
    /// single consumer thread on it. Returns `None` if already taken.
    pub fn take_event_receiver(&self) -> Option<Receiver<PaneEvent>> {
        self.rx.lock().take()
    }

    /// Register extra environment variables that will be injected into every
    /// subsequently spawned PTY process. Intended for things like `YMUX_IPC`.
    pub fn set_extra_env(&self, env: Vec<(String, String)>) {
        *self.extra_env.lock() = env;
    }

    pub fn spawn(
        &self,
        spec: &PaneSpec,
        profile: &ShellProfile,
        size: PtySize,
    ) -> YmuxResult<SpawnedPane> {
        let extra = self.extra_env.lock().clone();
        let session = PtySession::spawn(
            spec,
            profile,
            size,
            self.tx.clone(),
            Arc::clone(&self.cwds),
            &extra,
        )?;
        let id = session.id;
        let shell = profile.name.clone();
        self.sessions.lock().insert(id, session);
        Ok(SpawnedPane { id, shell })
    }

    /// Return the most recently reported working directory for `id`, if any.
    pub fn cwd_for(&self, id: Uuid) -> Option<String> {
        self.cwds.lock().get(&id).cloned()
    }

    /// Snapshot of the entire `pane id → cwd` map. Cheap clone used by
    /// `save_config` to patch the layout tree in one pass.
    pub fn cwds_snapshot(&self) -> HashMap<Uuid, String> {
        self.cwds.lock().clone()
    }

    pub fn write(&self, id: Uuid, data: &[u8]) -> YmuxResult<()> {
        let sessions = self.sessions.lock();
        let session = sessions.get(&id).ok_or(YmuxError::UnknownPane(id))?;
        session.write(data)
    }

    pub fn resize(&self, id: Uuid, size: PtySize) -> YmuxResult<()> {
        let sessions = self.sessions.lock();
        let session = sessions.get(&id).ok_or(YmuxError::UnknownPane(id))?;
        session.resize(size)
    }

    pub fn kill(&self, id: Uuid) -> YmuxResult<()> {
        // Remove under the lock so the Drop impl can run unguarded and join
        // the reader thread without deadlocking the manager.
        let session = self.sessions.lock().remove(&id);
        // Drop the dead pane's cached cwd so a later pane reusing the same
        // id doesn't inherit a stale directory.
        self.cwds.lock().remove(&id);
        match session {
            Some(s) => s.kill(),
            None => Err(YmuxError::UnknownPane(id)),
        }
    }

    pub fn has(&self, id: Uuid) -> bool {
        self.sessions.lock().contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.lock().is_empty()
    }

    /// Drop every session. Used on window close.
    pub fn shutdown_all(&self) {
        let mut map = self.sessions.lock();
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_dir_is_prepended() {
        let dir = Path::new("/Applications/ymux.app/Contents/MacOS");
        let current = std::env::join_paths([Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        let out = path_with_sidecar_dir(dir, Some(&current)).expect("some");
        let parts: Vec<PathBuf> = std::env::split_paths(&out).collect();
        // First, so a bundled tool beats a stale copy from an older install.
        assert_eq!(parts[0], dir);
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn already_present_dir_is_left_alone() {
        let dir = Path::new("/opt/ymux");
        let current = std::env::join_paths([Path::new("/usr/bin"), dir]).unwrap();
        // Returning None keeps PATH from growing an extra copy every launch
        // for users who also added the directory to their own dotfiles.
        assert!(path_with_sidecar_dir(dir, Some(&current)).is_none());
    }

    #[test]
    fn empty_path_still_yields_the_sidecar_dir() {
        let dir = Path::new("/opt/ymux");
        let out = path_with_sidecar_dir(dir, None).expect("some");
        let parts: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(parts, vec![dir.to_path_buf()]);
    }
}
