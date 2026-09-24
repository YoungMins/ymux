//! Owner of all [`PtySession`]s for the running app.
//!
//! The manager keeps a registry keyed by pane [`Uuid`] and centralises the
//! reader → frontend event channel. Each spawned pane pushes its bytes into a
//! single `mpsc` channel; the caller (the Tauri layer) drains it on a
//! dedicated thread and forwards events to the webview.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use parking_lot::Mutex;
use portable_pty::PtySize;
use uuid::Uuid;

use crate::config::model::{PaneSpec, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::pty::session::{CwdMap, PaneEvent, PtySession};

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
    // (`YMUX_HOOK_TOKEN`). Set once at startup, read on every spawn.
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
    /// subsequently spawned PTY process. Intended for things like
    /// `YMUX_HOOK_TOKEN`.
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

    /// Snapshot of `pane id → shell PID` for every live session whose PID is
    /// known. Read by the agent scan every 2 s.
    pub fn pids_snapshot(&self) -> HashMap<Uuid, u32> {
        self.sessions
            .lock()
            .iter()
            .filter_map(|(id, s)| s.pid().map(|p| (*id, p)))
            .collect()
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
    fn pids_snapshot_tracks_live_sessions() {
        let profile = if cfg!(windows) {
            ShellProfile {
                name: "cmd".into(),
                executable: "cmd.exe".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            }
        } else {
            ShellProfile {
                name: "sh".into(),
                executable: "/bin/sh".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            }
        };
        let mgr = PtyManager::default();
        let spec = PaneSpec::new_default();
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        mgr.spawn(&spec, &profile, size).expect("spawn");
        assert!(mgr.pids_snapshot().get(&spec.id).is_some_and(|p| *p > 0));
        mgr.kill(spec.id).expect("kill");
        assert!(!mgr.pids_snapshot().contains_key(&spec.id));
    }
}
