//! Dock mode (`ydir --dock`): one listing plus a preview, and — under ymux —
//! Enter on a file asks the host to open it in a viewer tab instead of
//! running ycode inline.
//!
//! ymux's own file dock no longer runs this: it is a GUI files pane now and
//! follows the active pane's directory itself, so the `ChangeDir` follower
//! that used to live here is gone. `--dock` stays for the one release in
//! which `ydir` is still on PATH (spec §5 step 2).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};

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

/// Connect to the ymux host and introduce ourselves under `tool`. `None` on
/// any failure — the dock keeps working, it just loses that one capability.
fn connect_client(address: &str, tool: &str, pane_id: String) -> Option<IpcClient> {
    let mut client = IpcClient::connect(address).ok()?;
    let hello = IpcMessage::Hello {
        tool: tool.into(),
        pane_id,
    };
    client.send(&hello).ok()?;
    Some(client)
}

/// Tool name the outbound `open-file` link introduces itself with.
const OPEN_FILE_TOOL: &str = "ydir-openfile";

/// Dock mode under ymux only: a channel whose paths are relayed to the host
/// as `open-file` events. `None` outside dock mode or outside ymux, which is
/// exactly what keeps a plain `ydir` launching ycode inline (spec §4).
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
    use std::time::Duration;
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
        // A second path over the same link: the ack drain keeps it usable.
        link.send(PathBuf::from("/w/한글.md")).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "/w/한글.md"
        );
        server.shutdown();
    }
}
