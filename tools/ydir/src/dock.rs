//! Dock mode. ymux runs `ydir --dock <dir>` in its right-side file dock and
//! pushes `ChangeDir` over yipc whenever the active pane's working directory
//! changes. A background thread forwards each one into the event loop
//! through a channel. A plain `ydir` never connects, so a yDir the user
//! opened in a pane is never steered by the dock.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use yipc::{IpcClient, IpcMessage};

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
            let Ok(mut client) = IpcClient::connect(&address) else {
                return;
            };
            let hello = IpcMessage::Hello {
                tool: "ydir".into(),
                pane_id,
            };
            if client.send(&hello).is_err() {
                return;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
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
    fn follow_host_is_off_outside_dock_mode_or_ymux() {
        assert!(follow_host(false, Some("tcp:127.0.0.1:1".into()), String::new()).is_none());
        assert!(follow_host(true, None, String::new()).is_none());
        assert!(follow_host(true, Some(String::new()), String::new()).is_none());
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
