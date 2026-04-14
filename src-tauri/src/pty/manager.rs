//! Owner of all [`PtySession`]s for the running app.
//!
//! The manager keeps a registry keyed by pane [`Uuid`] and centralises the
//! reader → frontend event channel. Each spawned pane pushes its bytes into a
//! single `mpsc` channel; the caller (the Tauri layer) drains it on a
//! dedicated thread and forwards events to the webview.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

use parking_lot::Mutex;
use portable_pty::PtySize;
use uuid::Uuid;

use crate::config::model::{PaneSpec, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::pty::session::{PaneEvent, PtySession};

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
}

impl Default for PtyManager {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            sessions: Mutex::new(HashMap::new()),
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }
}

impl PtyManager {
    /// Take ownership of the event receiver. The caller is expected to park a
    /// single consumer thread on it. Returns `None` if already taken.
    pub fn take_event_receiver(&self) -> Option<Receiver<PaneEvent>> {
        self.rx.lock().take()
    }

    pub fn spawn(
        &self,
        spec: &PaneSpec,
        profile: &ShellProfile,
        size: PtySize,
    ) -> YmuxResult<SpawnedPane> {
        let session = PtySession::spawn(spec, profile, size, self.tx.clone())?;
        let id = session.id;
        let shell = profile.name.clone();
        self.sessions.lock().insert(id, session);
        Ok(SpawnedPane { id, shell })
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
    fn manager_starts_empty() {
        let m = PtyManager::default();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn unknown_pane_returns_error() {
        let m = PtyManager::default();
        let id = Uuid::new_v4();
        assert!(matches!(m.write(id, b"x"), Err(YmuxError::UnknownPane(_))));
        assert!(matches!(
            m.resize(
                id,
                PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0
                }
            ),
            Err(YmuxError::UnknownPane(_))
        ));
        assert!(matches!(m.kill(id), Err(YmuxError::UnknownPane(_))));
    }

    #[test]
    fn receiver_can_only_be_taken_once() {
        let m = PtyManager::default();
        assert!(m.take_event_receiver().is_some());
        assert!(m.take_event_receiver().is_none());
    }
}
