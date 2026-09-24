//! IPC message definitions and serialization helpers.
//!
//! What is left after the GUI panes took over (spec §5 step 3): a tool says
//! `Hello`, sends `Event`s, and the host answers `Ack`. The one event kind in
//! use is [`AGENT_HOOK_KIND`], the Claude Code hook relay.

use serde::{Deserialize, Serialize};

/// `IpcMessage::Event::kind` used by `y agent-hook` to relay a coding-agent
/// hook to the ymux host (agent tree).
pub const AGENT_HOOK_KIND: &str = "agent-hook";

/// Messages exchanged between tools (clients) and the ymux host (server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum IpcMessage {
    /// Sent by a tool upon connection to identify itself.
    Hello { tool: String, pane_id: String },
    /// Generic event with arbitrary JSON payload.
    Event {
        kind: String,
        payload: serde_json::Value,
    },
    /// Acknowledgement from the server.
    Ack,
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
    fn roundtrip_hello() {
        let msg = IpcMessage::Hello {
            tool: "y".into(),
            pane_id: "abc-123".into(),
        };
        let line = msg.to_line().unwrap();
        assert!(line.ends_with('\n'));
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
    fn roundtrip_ack() {
        let msg = IpcMessage::Ack;
        let line = msg.to_line().unwrap();
        let decoded = IpcMessage::from_line(&line).unwrap();
        assert_eq!(msg, decoded);
    }

    /// A message type this build no longer knows (an old tool sending
    /// `RegisterCommands`) is a parse error, not a panic.
    #[test]
    fn a_retired_message_type_fails_to_parse() {
        assert!(IpcMessage::from_line(r#"{"type":"RegisterCommands","commands":[]}"#).is_err());
    }
}
