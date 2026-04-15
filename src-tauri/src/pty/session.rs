//! One `PtySession` = one running pseudo-terminal hosting a child process.
//!
//! Abstractions are provided by `portable-pty`, which wraps Windows ConPTY on
//! Windows and Unix openpty elsewhere. The same code compiles on both, which
//! lets us run unit tests on Linux.

use std::io::Read;
use std::io::Write as IoWrite;
use std::sync::mpsc::Sender;

use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use uuid::Uuid;

use crate::config::model::{PaneSpec, ShellProfile};
use crate::error::{YmuxError, YmuxResult};

/// Handle to a single running PTY. `stdout` bytes from the child are pushed
/// into a caller-provided `mpsc::Sender` on a dedicated reader thread — the
/// Tauri layer forwards them to the frontend via an event channel.
///
/// The reader thread is intentionally **detached** rather than joined on
/// drop. On Windows ConPTY the master read loop can stay blocked in `read()`
/// for an unbounded amount of time after the child has been killed, because
/// ConPTY does not necessarily close the master side immediately. Joining
/// that thread from `Drop` would freeze whichever Tauri command worker
/// happened to call `kill_pane`, which in turn deadlocks the entire IPC
/// surface and produces a "Not Responding" window the moment the user closes
/// a pane.
pub struct PtySession {
    pub id: Uuid,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn IoWrite + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

/// Event emitted from the reader thread back to the app layer.
#[derive(Debug, Clone)]
pub enum PaneEvent {
    /// Raw bytes written by the child to its stdout/stderr.
    Data(Uuid, Vec<u8>),
    /// Child has exited with the given status code (0 if unknown).
    Exit(Uuid, u32),
}

impl PtySession {
    /// Spawn the shell described by `profile` under a fresh ConPTY and wire
    /// its output into `events`.
    pub fn spawn(
        spec: &PaneSpec,
        profile: &ShellProfile,
        size: PtySize,
        events: Sender<PaneEvent>,
    ) -> YmuxResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| YmuxError::Pty(format!("openpty: {e}")))?;

        let mut cmd = CommandBuilder::new(&profile.executable);
        for a in &profile.args {
            cmd.arg(a);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| YmuxError::Pty(format!("spawn: {e}")))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| YmuxError::Pty(format!("take_writer: {e}")))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| YmuxError::Pty(format!("clone_reader: {e}")))?;

        let id = spec.id;
        let tx = events.clone();
        // Detached reader thread — we never join it. See the doc comment on
        // `PtySession` for why joining causes UI hangs on Windows.
        std::thread::Builder::new()
            .name(format!("ymux-pty-reader-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            if tx.send(PaneEvent::Data(id, buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::debug!(pane = %id, error = %e, "pty reader error");
                            break;
                        }
                    }
                }
                // Signal exit once the reader drains. The receiver may have
                // already gone away if the pane was disposed, in which case
                // the send fails silently and that's fine.
                let _ = tx.send(PaneEvent::Exit(id, 0));
            })
            .map_err(|e| YmuxError::Pty(format!("spawn reader thread: {e}")))?;

        // Drop the slave so the child inherits it and closing the master
        // actually reaches EOF. `portable-pty` drops it when `pair.slave` goes
        // out of scope here.
        drop(pair.slave);

        Ok(Self {
            id,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
        })
    }

    pub fn write(&self, data: &[u8]) -> YmuxResult<()> {
        let mut w = self.writer.lock();
        w.write_all(data)
            .map_err(|e| YmuxError::Pty(format!("write: {e}")))?;
        w.flush()
            .map_err(|e| YmuxError::Pty(format!("flush: {e}")))?;
        Ok(())
    }

    pub fn resize(&self, size: PtySize) -> YmuxResult<()> {
        self.master
            .lock()
            .resize(size)
            .map_err(|e| YmuxError::Pty(format!("resize: {e}")))
    }

    /// Attempt to terminate the child process. Best-effort.
    pub fn kill(&self) -> YmuxResult<()> {
        let mut c = self.child.lock();
        c.kill().map_err(|e| YmuxError::Pty(format!("kill: {e}")))?;
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // Best effort: terminate the child. The reader thread will see EOF
        // (or an error) on its next `read()` and exit on its own. We do NOT
        // join the reader here because on Windows ConPTY the master read can
        // stay blocked indefinitely after the child has been killed, and
        // joining would freeze the calling Tauri command worker thread,
        // hanging the whole IPC surface and causing "Not Responding" the
        // moment the user closes a pane.
        let _ = self.child.lock().kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[cfg(unix)]
    #[test]
    fn pty_spawn_echo_roundtrip() {
        // Sanity check on unix: spawn `sh`, send `echo hello`, observe the
        // output. Validates that PtySession wiring is correct end-to-end. On
        // Windows this test would use cmd.exe, but we only run it in the Linux
        // dev sandbox.
        let profile = ShellProfile {
            name: "sh".into(),
            executable: "/bin/sh".into(),
            args: vec![],
            icon: None,
            color: None,
        };
        if !std::path::Path::new(&profile.executable).exists() {
            eprintln!("skipping: /bin/sh not present");
            return;
        }
        let spec = PaneSpec::new_default();
        let (tx, rx) = mpsc::channel();
        let session = PtySession::spawn(
            &spec,
            &profile,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            tx,
        )
        .expect("spawn");
        session
            .write(b"echo ymux-test-marker\nexit\n")
            .expect("write");

        let mut captured = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(PaneEvent::Data(_, b)) => captured.extend_from_slice(&b),
                Ok(PaneEvent::Exit(_, _)) => break,
                Err(_) if std::time::Instant::now() > deadline => break,
                Err(_) => continue,
            }
        }
        let text = String::from_utf8_lossy(&captured);
        assert!(
            text.contains("ymux-test-marker"),
            "expected marker in output, got: {text:?}"
        );
    }
}
