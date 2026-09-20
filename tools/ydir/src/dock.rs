//! Dock mode. ymux runs `ydir --dock <dir>` in its right-side file dock and
//! pushes `ChangeDir` over yipc whenever the active pane's working directory
//! changes. A background thread forwards each one into the event loop
//! through a channel. A plain `ydir` never connects, so a yDir the user
//! opened in a pane is never steered by the dock.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use yipc::{IpcClient, IpcMessage};

/// How long the keyboard must be idle before a pushed directory is applied.
pub const QUIET: Duration = Duration::from_millis(300);

/// A `ChangeDir` waiting for a safe moment to land.
///
/// Swapping the listing under a user who is mid-keystroke turns their next
/// key into an action on a different file: `d` deletes without asking, and
/// it would delete row 0 of the new folder. So a pushed directory waits
/// until nothing is queued in the input and no key has arrived for
/// [`QUIET`]. Only the latest one is kept, because the host only ever
/// cares about where the active pane is now.
#[derive(Debug, Default)]
pub struct PendingDir {
    dir: Option<PathBuf>,
    last_key: Option<Instant>,
}

impl PendingDir {
    pub fn push(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    pub fn on_key(&mut self, now: Instant) {
        self.last_key = Some(now);
    }

    /// The directory to apply now, if it is safe to. `input_pending` is
    /// whether an input event is already waiting to be read.
    pub fn take_ready(&mut self, now: Instant, input_pending: bool) -> Option<PathBuf> {
        if input_pending {
            return None;
        }
        if let Some(last) = self.last_key {
            if now.saturating_duration_since(last) < QUIET {
                return None;
            }
        }
        self.dir.take()
    }
}

/// Command line: `ydir [--dock] [DIR]`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub dock: bool,
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    for arg in args {
        if arg == "--dock" {
            out.dock = true;
        } else if out.dir.is_none() {
            out.dir = Some(PathBuf::from(arg));
        }
    }
    out
}

/// In dock mode under ymux (`address` = `$YMUX_IPC`), connect on a
/// background thread, say `Hello { tool: "ydir" }`, and forward every
/// `ChangeDir` into the returned channel. Returns `None` when there is
/// nothing to follow, which keeps ydir exactly as it was before dock mode.
/// A failed connect just ends the thread. The dock keeps working, it just
/// stops following.
/// Connect to the ymux host and introduce ourselves under `tool`. `None` on
/// any failure — the dock keeps working, it just loses that one capability.
///
/// The name matters: the host's `send_to(tool, …)` fans a message out to
/// *every* client registered under it, so the two connections this module
/// opens must not share one. Only the `ChangeDir` follower answers to
/// `"ydir"`; the outbound `open-file` link registers under its own name and
/// therefore never has a push queued into a socket it does not read.
fn connect_client(address: &str, tool: &str, pane_id: String) -> Option<IpcClient> {
    let mut client = IpcClient::connect(address).ok()?;
    let hello = IpcMessage::Hello {
        tool: tool.into(),
        pane_id,
    };
    client.send(&hello).ok()?;
    Some(client)
}

/// Tool name the outbound `open-file` link registers under. Deliberately not
/// `"ydir"` — see [`connect_client`].
const OPEN_FILE_TOOL: &str = "ydir-openfile";

pub fn follow_host(
    dock: bool,
    address: Option<String>,
    pane_id: String,
) -> Option<Receiver<PathBuf>> {
    if !dock {
        return None;
    }
    let address = address.filter(|a| !a.is_empty())?;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("ydir-ipc".into())
        .spawn(move || {
            let Some(mut client) = connect_client(&address, "ydir", pane_id) else {
                return;
            };
            while let Ok(msg) = client.recv() {
                if let IpcMessage::ChangeDir { path } = msg {
                    if tx.send(PathBuf::from(path)).is_err() {
                        break; // the event loop is gone
                    }
                }
            }
        })
        .ok()?;
    Some(rx)
}

/// Dock mode under ymux only: a channel whose paths are relayed to the host
/// as `open-file` events. `None` outside dock mode or outside ymux, which is
/// exactly what keeps a plain `ydir` launching ycode inline (spec §4).
///
/// A second connection, not a reuse of `follow_host`'s: that one is parked in
/// a blocking `recv()`. The server accepts many clients, and either
/// connection failing leaves the other intact. It registers as
/// [`OPEN_FILE_TOOL`], so the host's `ChangeDir` pushes to `"ydir"` never
/// pile up unread in this socket.
pub fn open_file_link(
    dock: bool,
    address: Option<String>,
    pane_id: String,
) -> Option<Sender<PathBuf>> {
    if !dock {
        return None;
    }
    let address = address.filter(|a| !a.is_empty())?;
    let (tx, rx) = channel::<PathBuf>();
    std::thread::Builder::new()
        .name("ydir-openfile".into())
        .spawn(move || {
            let Some(mut client) = connect_client(&address, OPEN_FILE_TOOL, pane_id) else {
                return;
            };
            while let Ok(path) = rx.recv() {
                let msg = yipc::open_file_event(&path.to_string_lossy());
                if client.send(&msg).is_err() {
                    break; // the host is gone
                }
                // The host always acks; drain it so the socket doesn't fill.
                let _ = client.recv();
            }
        })
        .ok()?;
    Some(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yipc::{IpcMessage, IpcServer, MessageHandler};

    fn args(list: &[&str]) -> Args {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_args_keeps_the_plain_dir_argument() {
        assert_eq!(args(&[]), Args::default());
        assert_eq!(
            args(&["/work"]),
            Args {
                dir: Some(PathBuf::from("/work")),
                dock: false
            }
        );
    }

    #[test]
    fn parse_args_accepts_dock_before_or_after_the_dir() {
        let want = Args {
            dir: Some(PathBuf::from("/work")),
            dock: true,
        };
        assert_eq!(args(&["--dock", "/work"]), want);
        assert_eq!(args(&["/work", "--dock"]), want);
    }

    #[test]
    fn open_file_link_is_off_outside_dock_mode_or_ymux() {
        assert!(open_file_link(false, Some("tcp:127.0.0.1:1".into()), String::new()).is_none());
        assert!(open_file_link(true, None, String::new()).is_none());
        assert!(open_file_link(true, Some(String::new()), String::new()).is_none());
    }

    #[test]
    fn open_file_link_relays_a_path_to_the_host_as_an_open_file_event() {
        let (tx, rx) = channel::<String>();
        let handler: MessageHandler = Box::new(move |msg, writer| {
            if let Some(path) = yipc::open_file_path(&msg) {
                let _ = tx.send(path.to_string());
            }
            writer
                .write_all(IpcMessage::Ack.to_line().unwrap().as_bytes())
                .ok();
            writer.flush().ok();
        });
        let server = IpcServer::start(handler).unwrap();
        let link = open_file_link(true, Some(server.address().to_string()), "dock".into())
            .expect("dock mode with an address gets a link");

        link.send(PathBuf::from("/w/README.md")).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "/w/README.md"
        );

        // The link is registered by now (it just spoke), and it must NOT be
        // registered as "ydir": `send_to` fans out to every client under a
        // name, and this connection reads only its own acks, so a ChangeDir
        // landing here would sit unread until the host's write timeout
        // killed the socket and dock-Enter stopped working.
        assert_eq!(
            server
                .send_to(
                    "ydir",
                    &IpcMessage::ChangeDir {
                        path: "/elsewhere".into()
                    }
                )
                .unwrap(),
            0,
            "the open-file link must not answer to the follower's tool name"
        );
        server.shutdown();
    }

    #[test]
    fn follow_host_is_off_outside_dock_mode_or_ymux() {
        assert!(follow_host(false, Some("tcp:127.0.0.1:1".into()), String::new()).is_none());
        assert!(follow_host(true, None, String::new()).is_none());
        assert!(follow_host(true, Some(String::new()), String::new()).is_none());
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn pending_dir_applies_when_the_user_is_idle() {
        let t0 = Instant::now();
        let mut p = PendingDir::default();
        p.push(PathBuf::from("/a"));
        // No key ever pressed, nothing queued: apply right away.
        assert_eq!(p.take_ready(t0, false), Some(PathBuf::from("/a")));
        assert_eq!(p.take_ready(t0, false), None);
    }

    #[test]
    fn pending_dir_waits_for_a_quiet_period_after_a_key() {
        let t0 = Instant::now();
        let mut p = PendingDir::default();
        p.on_key(t0);
        p.push(PathBuf::from("/a"));
        assert_eq!(p.take_ready(t0 + ms(100), false), None);
        assert_eq!(p.take_ready(t0 + QUIET - ms(1), false), None);
        // Another key restarts the quiet period.
        p.on_key(t0 + ms(250));
        assert_eq!(p.take_ready(t0 + QUIET, false), None);
        assert_eq!(
            p.take_ready(t0 + ms(250) + QUIET, false),
            Some(PathBuf::from("/a"))
        );
    }

    #[test]
    fn pending_dir_keeps_only_the_latest_target() {
        let t0 = Instant::now();
        let mut p = PendingDir::default();
        p.on_key(t0);
        p.push(PathBuf::from("/a"));
        p.push(PathBuf::from("/b"));
        assert_eq!(p.take_ready(t0 + QUIET, false), Some(PathBuf::from("/b")));
        assert_eq!(p.take_ready(t0 + QUIET * 2, false), None);
    }

    #[test]
    fn pending_dir_never_jumps_ahead_of_queued_input() {
        // A key already sitting in the input queue was typed against the
        // listing on screen, so it has to be handled before the jump.
        let t0 = Instant::now();
        let mut p = PendingDir::default();
        p.push(PathBuf::from("/a"));
        assert_eq!(p.take_ready(t0, true), None);
        assert_eq!(p.take_ready(t0, false), Some(PathBuf::from("/a")));
    }

    #[test]
    fn follow_host_forwards_change_dir_into_the_channel() {
        let handler: MessageHandler = Box::new(|_msg, writer| {
            writer
                .write_all(IpcMessage::Ack.to_line().unwrap().as_bytes())
                .ok();
            writer.flush().ok();
        });
        let server = IpcServer::start(handler).unwrap();
        let rx = follow_host(true, Some(server.address().to_string()), "dock".into())
            .expect("dock mode with an address follows the host");

        let msg = IpcMessage::ChangeDir {
            path: "/somewhere".into(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.send_to("ydir", &msg).unwrap() == 0 {
            assert!(
                Instant::now() < deadline,
                "ydir never registered with the host"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            PathBuf::from("/somewhere")
        );
        server.shutdown();
    }
}
