//! One `PtySession` = one running pseudo-terminal hosting a child process.
//!
//! Abstractions are provided by `portable-pty`, which wraps Windows ConPTY on
//! Windows and Unix openpty elsewhere. The same code compiles on both, which
//! lets us run unit tests on Linux.

use std::collections::HashMap;
use std::io::Read;
use std::io::Write as IoWrite;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use uuid::Uuid;

use crate::config::model::{PaneSpec, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::pty::osc7::Osc7Parser;

/// Shared map of pane id → latest known current working directory. Populated
/// by per-session reader threads as they parse OSC 7 sequences out of the
/// PTY output stream; read by `save_config` to patch the layout tree before
/// persisting it.
pub type CwdMap = Arc<Mutex<HashMap<Uuid, String>>>;

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
    /// its output into `events`. The reader thread also feeds bytes through
    /// an OSC 7 parser and writes any detected working directory into
    /// `cwds` under this pane's id.
    pub fn spawn(
        spec: &PaneSpec,
        profile: &ShellProfile,
        size: PtySize,
        events: Sender<PaneEvent>,
        cwds: CwdMap,
        extra_env: &[(String, String)],
    ) -> YmuxResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| YmuxError::Pty(format!("openpty: {e}")))?;

        let mut cmd = CommandBuilder::new(&profile.executable);
        for a in &profile.args {
            cmd.arg(a);
        }

        // ymux *is* the terminal emulator, so it owes the child a `TERM`.
        // Nothing else sets one: `portable-pty` doesn't, and a GUI-launched
        // `.app` inherits launchd's environment, which has no `TERM` at all.
        // Without it zsh, ls, git and friends all decide they're not talking
        // to a terminal and disable colour, so every pane renders monochrome.
        // Set before the profile and pane env below so either can override.
        #[cfg(unix)]
        {
            cmd.env("TERM", "xterm-256color");
            cmd.env("COLORTERM", "truecolor");
        }

        // Fall back to the home directory when the pane has no usable cwd —
        // either it never recorded one, or the directory has since been
        // deleted. Inheriting ymux's own cwd instead lands the shell wherever
        // the OS happened to start the app, which for a `.app` opened from
        // Finder is `/`.
        let cwd = spec
            .cwd
            .as_deref()
            .filter(|p| std::path::Path::new(p).is_dir())
            .map(|p| p.to_string())
            .or_else(|| dirs::home_dir().map(|p| p.display().to_string()));
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }
        // Shell-profile env first (e.g. the macOS zsh profile's ZDOTDIR shim),
        // then the pane's own env, so a pane can deliberately override
        // anything the profile set.
        for (k, v) in &profile.env {
            cmd.env(k, v);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        // Inject manager-level env vars (e.g. YMUX_IPC).
        for (k, v) in extra_env {
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
        let cwds_for_reader = Arc::clone(&cwds);
        // Detached reader thread — we never join it. See the doc comment on
        // `PtySession` for why joining causes UI hangs on Windows.
        std::thread::Builder::new()
            .name(format!("ymux-pty-reader-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut osc7 = Osc7Parser::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            let chunk = &buf[..n];
                            // Parse any OSC 7 cwd reports hiding inside the
                            // chunk and update the shared map. The last one
                            // wins — that's the "current" cwd as far as the
                            // shell is concerned.
                            for cwd in osc7.feed(chunk) {
                                cwds_for_reader.lock().insert(id, cwd);
                            }
                            if tx.send(PaneEvent::Data(id, chunk.to_vec())).is_err() {
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
            env: Vec::new(),
        };
        if !std::path::Path::new(&profile.executable).exists() {
            eprintln!("skipping: /bin/sh not present");
            return;
        }
        let spec = PaneSpec::new_default();
        let (tx, rx) = mpsc::channel();
        let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
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
            Arc::clone(&cwds),
            &[],
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

    /// A pane with no inherited `TERM` must still get one.
    ///
    /// Nothing upstream sets it — `portable-pty` doesn't, and a GUI-launched
    /// `.app` inherits launchd's environment, which has none. Without `TERM`
    /// every colour-capable tool in the pane (ls, git, grep, the prompt)
    /// decides it isn't talking to a terminal and renders monochrome, which
    /// is exactly how this surfaced: a whole terminal in plain white.
    #[cfg(unix)]
    #[test]
    fn pty_child_gets_a_term_even_when_the_parent_has_none() {
        let profile = ShellProfile {
            name: "sh".into(),
            executable: "/bin/sh".into(),
            args: vec![],
            icon: None,
            color: None,
            env: Vec::new(),
        };
        if !std::path::Path::new(&profile.executable).exists() {
            eprintln!("skipping: /bin/sh not present");
            return;
        }
        // The test binary itself usually has TERM set; drop it so this
        // reproduces the Finder-launch environment rather than the shell one.
        std::env::remove_var("TERM");
        std::env::remove_var("COLORTERM");

        let spec = PaneSpec::new_default();
        let (tx, rx) = mpsc::channel();
        let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
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
            Arc::clone(&cwds),
            &[],
        )
        .expect("spawn");
        session
            .write(b"printf 'ymux-term=[%s]\n' \"$TERM\"\nexit\n")
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
            text.contains("ymux-term=[xterm-256color]"),
            "child should see TERM=xterm-256color, got: {text:?}"
        );
    }

    /// A pane whose recorded cwd is gone — or was never recorded — must land
    /// in the home directory, not wherever the OS started ymux. For a `.app`
    /// opened from Finder that inherited directory is `/`, which is how this
    /// showed up: every pane reopening at the filesystem root.
    #[cfg(unix)]
    #[test]
    fn pty_falls_back_to_home_when_cwd_is_missing_or_stale() {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        let profile = ShellProfile {
            name: "sh".into(),
            executable: "/bin/sh".into(),
            args: vec![],
            icon: None,
            color: None,
            env: Vec::new(),
        };
        if !std::path::Path::new(&profile.executable).exists() {
            return;
        }

        for cwd in [None, Some("/definitely/not/a/real/directory".to_string())] {
            let mut spec = PaneSpec::new_default();
            spec.cwd = cwd.clone();
            let (tx, rx) = mpsc::channel();
            let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
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
                Arc::clone(&cwds),
                &[],
            )
            .unwrap_or_else(|e| panic!("spawn with cwd {cwd:?} failed: {e}"));
            session
                .write(b"printf 'ymux-pwd=[%s]\n' \"$PWD\"\nexit\n")
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
            let expected = format!("ymux-pwd=[{}]", home.display());
            assert!(
                text.contains(&expected),
                "cwd {cwd:?} should fall back to {expected}, got: {text:?}"
            );
        }
    }

    /// End-to-end check that the macOS shell integration actually reports a
    /// live cwd: spawn a *detected* profile (so the zsh ZDOTDIR shim / bash
    /// `--rcfile` really is in play), `cd` somewhere, and assert the OSC 7
    /// reader wrote the new directory into the shared cwd map.
    ///
    /// This is what makes "split inherits the parent's current directory"
    /// work, and it silently degrades to the startup dir if the shim breaks —
    /// exactly the kind of regression a unit test on `detect_shells()` alone
    /// would not catch.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_shell_integration_reports_live_cwd() {
        // `/tmp` is a symlink to `/private/tmp` on macOS and shells report
        // `$PWD` with the symlink intact, so canonicalize both sides rather
        // than assuming either form.
        let target = std::fs::canonicalize("/tmp").expect("canonicalize /tmp");
        let same = |cwd: &str| {
            std::fs::canonicalize(cwd)
                .map(|p| p == target)
                .unwrap_or(false)
        };

        for profile in crate::shell::detect_shells() {
            // Every macOS profile carries a shell integration: zsh via
            // ZDOTDIR, bash via --rcfile, POSIX shells via $ENV, and fish
            // natively. Assert that rather than skipping anything, so a
            // profile that quietly loses its hook fails the test.
            let integrated = profile
                .env
                .iter()
                .any(|(k, _)| k == "ZDOTDIR" || k == "ENV")
                || profile.args.iter().any(|a| a == "--rcfile")
                || profile.executable.ends_with("/fish");
            assert!(
                integrated,
                "{} has no shell integration, so it can never report a cwd",
                profile.name
            );
            let spec = PaneSpec::new_default();
            let (tx, rx) = mpsc::channel();
            let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
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
                Arc::clone(&cwds),
                &[],
            )
            .expect("spawn");

            // The trailing `echo` forces one more prompt — and therefore one
            // more precmd/PROMPT_COMMAND run — after the `cd`.
            session
                .write(b"cd /tmp\necho ymux-cwd-probe\n")
                .expect("write");

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut got = None;
            while std::time::Instant::now() < deadline {
                // Drain events so the reader thread keeps making progress.
                let _ = rx.recv_timeout(std::time::Duration::from_millis(200));
                if let Some(cwd) = cwds.lock().get(&spec.id).cloned() {
                    if same(&cwd) {
                        got = Some(cwd);
                        break;
                    }
                }
            }
            let _ = session.write(b"exit\n");

            assert!(
                got.is_some(),
                "{} did not report /tmp via OSC 7 (last seen: {:?})",
                profile.name,
                cwds.lock().get(&spec.id).cloned()
            );
        }
    }
}
