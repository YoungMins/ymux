//! IPC message definitions and serialization helpers.

use serde::{Deserialize, Serialize};

/// A single command that a tool can register with ymux.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandDef {
    pub id: String,
    pub label: String,
}

/// `IpcMessage::Event::kind` used by `y agent-hook` to relay a coding-agent
/// hook to the ymux host (agent tree).
pub const AGENT_HOOK_KIND: &str = "agent-hook";

/// `IpcMessage::Event::kind` used by the file dock's yDir to ask ymux to open
/// a file. ymux answers by putting it in the viewer tab of the pane the dock
/// follows; plain `ydir` never sends this and still runs ycode inline.
pub const OPEN_FILE_KIND: &str = "open-file";

/// Messages exchanged between tools (clients) and the ymux host (server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum IpcMessage {
    /// Sent by a tool upon connection to identify itself.
    Hello { tool: String, pane_id: String },
    /// Register commands that the tool exposes to the palette.
    RegisterCommands { commands: Vec<CommandDef> },
    /// Send opaque data to another pane by id.
    PaneSend { target: String, data: Vec<u8> },
    /// Generic event with arbitrary JSON payload.
    Event {
        kind: String,
        payload: serde_json::Value,
    },
    /// Host → tool: navigate to `path`. Sent by ymux to the dock's yDir when
    /// the active pane's working directory changes.
    ChangeDir { path: String },
    /// Acknowledgement from the server.
    Ack,
}

/// Build the `open-file` event for `path`.
pub fn open_file_event(path: &str) -> IpcMessage {
    IpcMessage::Event {
        kind: OPEN_FILE_KIND.to_string(),
        payload: serde_json::json!({ "path": path }),
    }
}

/// The path carried by an `open-file` event, or `None` for any other message
/// (and for an `open-file` whose payload is malformed).
pub fn open_file_path(msg: &IpcMessage) -> Option<&str> {
    match msg {
        IpcMessage::Event { kind, payload } if kind == OPEN_FILE_KIND => {
            payload.get("path")?.as_str()
        }
        _ => None,
    }
}

impl IpcMessage {
    /// Serialize to a newline-terminated JSON string.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Deserialize from a single JSON line (trailing newline optional).
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim_end_matches('\n'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_file_event_roundtrips_and_is_readable() {
        let msg = open_file_event(r"D:\Git\ymux\src\main.ts");
        let line = msg.to_line().unwrap();
        assert!(line.contains(r#""kind":"open-file""#), "got {line}");
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
        assert_eq!(open_file_path(&decoded), Some(r"D:\Git\ymux\src\main.ts"));
    }

    #[test]
    fn open_file_path_ignores_everything_else() {
        assert_eq!(open_file_path(&IpcMessage::Ack), None);
        assert_eq!(
            open_file_path(&IpcMessage::ChangeDir { path: "/x".into() }),
            None
        );
        // Another tool's event, and a malformed payload of our own kind.
        assert_eq!(
            open_file_path(&IpcMessage::Event {
                kind: "agent-hook".into(),
                payload: serde_json::json!({ "path": "/x" }),
            }),
            None
        );
        assert_eq!(
            open_file_path(&IpcMessage::Event {
                kind: OPEN_FILE_KIND.into(),
                payload: serde_json::json!({ "path": 7 }),
            }),
            None
        );
    }

    #[test]
    fn roundtrip_hello() {
        let msg = IpcMessage::Hello {
            tool: "ymon".into(),
            pane_id: "abc-123".into(),
        };
        let line = msg.to_line().unwrap();
        assert!(line.ends_with('\n'));
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn roundtrip_register_commands() {
        let msg = IpcMessage::RegisterCommands {
            commands: vec![
                CommandDef {
                    id: "restart".into(),
                    label: "Restart Service".into(),
                },
                CommandDef {
                    id: "stop".into(),
                    label: "Stop Service".into(),
                },
            ],
        };
        let line = msg.to_line().unwrap();
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn roundtrip_pane_send() {
        let msg = IpcMessage::PaneSend {
            target: "pane-42".into(),
            data: vec![0, 1, 2, 255],
        };
        let line = msg.to_line().unwrap();
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn roundtrip_event() {
        let msg = IpcMessage::Event {
            kind: "file_changed".into(),
            payload: serde_json::json!({"path": "/tmp/foo.txt"}),
        };
        let line = msg.to_line().unwrap();
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn roundtrip_change_dir() {
        let msg = IpcMessage::ChangeDir {
            path: r"C:\Users\me\src".into(),
        };
        let line = msg.to_line().unwrap();
        assert!(line.contains(r#""type":"ChangeDir""#), "got {line}");
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn roundtrip_ack() {
        let msg = IpcMessage::Ack;
        let line = msg.to_line().unwrap();
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }
}
