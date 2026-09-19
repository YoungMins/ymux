//! `y agent-hook <agent>` — relays a coding agent's hook event to ymux.
//!
//! Claude Code runs this for every hook ymux installed (see
//! `src-tauri/src/agent_hooks.rs`), with the hook JSON on stdin. It must never
//! get in Claude's way: it prints nothing, always exits 0, and returns at once
//! outside ymux (no `YMUX_PANE_ID` / `YMUX_IPC`), so Claude sessions started
//! in any other terminal are untouched.

use std::io::Read;
use std::time::Duration;

use serde_json::{Map, Value};
use yipc::{IpcClient, IpcMessage, AGENT_HOOK_KIND};

/// Upper bound on waiting for the host's Ack.
const ACK_TIMEOUT: Duration = Duration::from_millis(300);

/// Optional hook fields forwarded when present.
const OPTIONAL_FIELDS: &[&str] = &["agent_id", "agent_type", "tool_name"];

/// Build the IPC event for one hook invocation, or `None` if `stdin_json`
/// isn't a hook payload.
pub fn hook_message(agent: &str, stdin_json: &str, pane_id: &str) -> Option<IpcMessage> {
    let input: Value = serde_json::from_str(stdin_json).ok()?;
    let event = input.get("hook_event_name")?.as_str()?;
    let session_id = input
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut payload = Map::new();
    payload.insert("pane_id".into(), pane_id.into());
    payload.insert("agent".into(), agent.into());
    payload.insert("event".into(), event.into());
    payload.insert("session_id".into(), session_id.into());
    for key in OPTIONAL_FIELDS {
        if let Some(v) = input.get(*key).and_then(Value::as_str) {
            payload.insert((*key).into(), v.into());
        }
    }
    Some(IpcMessage::Event {
        kind: AGENT_HOOK_KIND.into(),
        payload: Value::Object(payload),
    })
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Entry point for `y agent-hook <agent> [--ymux-agent-hook]`. Never fails,
/// never prints; the caller exits 0 afterwards.
pub fn run(args: &[String]) {
    let (Some(pane_id), Some(ipc)) = (env_nonempty("YMUX_PANE_ID"), env_nonempty("YMUX_IPC"))
    else {
        return;
    };
    let agent = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("claude");
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Some(msg) = hook_message(agent, &input, &pane_id) else {
        return;
    };
    let Ok(mut client) = IpcClient::connect(&ipc) else {
        return;
    };
    if client.send(&msg).is_err() {
        return;
    }
    // Wait (bounded) for the Ack so the host has read our line before we
    // close. On Windows, closing with a reply still in flight can reset the
    // connection and drop the message.
    if client.set_read_timeout(Some(ACK_TIMEOUT)).is_ok() {
        let _ = client.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_the_hook_fields_ymux_needs() {
        let input = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
                        "agent_id":"a1","agent_type":"Explore","tool_input":{"command":"ls"}}"#;
        let msg = hook_message("claude", input, "pane-1").expect("message");
        assert_eq!(
            msg,
            IpcMessage::Event {
                kind: AGENT_HOOK_KIND.into(),
                payload: serde_json::json!({
                    "pane_id": "pane-1", "agent": "claude", "event": "PreToolUse",
                    "session_id": "s1", "agent_id": "a1", "agent_type": "Explore",
                    "tool_name": "Bash"
                }),
            }
        );
    }

    #[test]
    fn omits_absent_optional_fields() {
        let msg = hook_message("claude", r#"{"hook_event_name":"Stop"}"#, "p").expect("message");
        let IpcMessage::Event { payload, .. } = msg else {
            panic!("not an event")
        };
        assert_eq!(
            payload,
            serde_json::json!({ "pane_id": "p", "agent": "claude", "event": "Stop", "session_id": "" })
        );
    }

    #[test]
    fn rejects_non_hook_input() {
        assert!(hook_message("claude", "not json", "p").is_none());
        assert!(hook_message("claude", r#"{"session_id":"s"}"#, "p").is_none());
    }
}
