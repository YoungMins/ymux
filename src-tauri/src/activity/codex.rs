//! Codex rollout JSONL lines -> [`CodexItem`].
//!
//! Shapes (observed in `~/.codex/sessions/**/rollout-*.jsonl`, schema is not
//! documented and may drift, so every field is optional):
//!
//! ```text
//! {"type":"event_msg","payload":{"type":"task_started","turn_id":"..","started_at":1791978621}}
//! {"type":"event_msg","payload":{"type":"task_complete","turn_id":"..","duration_ms":55061,"last_agent_message":".."}}
//! {"type":"event_msg","payload":{"type":"item_completed","turn_id":"..","item":
//!    {"type":"CommandExecution","command":["bash","-lc","cargo test"],"cwd":"..",
//!     "exit_code":0,"duration":{"secs":0,"nanos":209702600},"aggregated_output":".."}}}
//! {"type":"event_msg","payload":{"type":"item_completed","turn_id":"..","item":
//!    {"type":"FileChange","changes":{"<path>":{..}}}}}
//! ```
//!
//! Anything else (including malformed JSON) is `None`. File contents and
//! full output are never kept: only paths and a clipped output tail.

use super::extract::{
    MAX_CMD_CHARS, MAX_MESSAGE_CHARS, MAX_OUTPUT_TAIL_CHARS, MAX_PATHS, MAX_PATH_CHARS, WINDOW,
};
use super::redact::{clip, first_line_clipped, tail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexItem {
    TaskStarted {
        turn_id: String,
        /// Unix seconds, as Codex writes it.
        started_at: Option<i64>,
    },
    TaskComplete {
        turn_id: String,
        duration_ms: Option<u64>,
        last_agent_message: Option<String>,
    },
    Command {
        turn_id: String,
        command: String,
        cwd: String,
        exit_code: Option<i64>,
        duration_ms: Option<u64>,
        output_tail: Option<String>,
    },
    FileChange {
        turn_id: String,
        paths: Vec<String>,
    },
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// `{"secs":0,"nanos":209702600}` (or a bare millisecond number) -> ms.
fn duration_ms(v: &Value) -> Option<u64> {
    if let Some(ms) = v.as_u64() {
        return Some(ms);
    }
    let secs = v.get("secs")?.as_u64()?;
    let nanos = v.get("nanos").and_then(Value::as_u64).unwrap_or(0);
    secs.checked_mul(1000)?.checked_add(nanos / 1_000_000)
}

/// `["bash","-lc","cargo test"]` -> `cargo test`; other argv joined by spaces.
fn command_text(v: &Value) -> String {
    let joined = match v {
        Value::String(c) => c.clone(),
        Value::Array(a) => {
            let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            match parts.as_slice() {
                [_, flag, script] if matches!(*flag, "-lc" | "-c" | "/c" | "-Command") => {
                    (*script).to_owned()
                }
                _ => parts.join(" "),
            }
        }
        _ => String::new(),
    };
    first_line_clipped(&joined, MAX_CMD_CHARS * WINDOW)
}

fn change_paths(changes: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |p: &str| {
        if out.len() < MAX_PATHS && !p.is_empty() {
            let p = clip(p, MAX_PATH_CHARS);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    };
    match changes {
        Value::Object(map) => map.keys().for_each(|k| push(k)),
        Value::Array(items) => {
            for it in items {
                if let Some(p) = it
                    .get("path")
                    .or_else(|| it.get("file_path"))
                    .and_then(Value::as_str)
                {
                    push(p);
                }
            }
        }
        _ => {}
    }
    out
}

/// Parse one rollout line. `None` for anything not recorded.
pub fn parse_line(line: &[u8]) -> Option<CodexItem> {
    let root: Value = serde_json::from_slice(line).ok()?;
    if root.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    let p = root.get("payload")?;
    let turn_id = s(p, "turn_id").unwrap_or_default();
    match p.get("type")?.as_str()? {
        "task_started" => Some(CodexItem::TaskStarted {
            turn_id,
            started_at: p.get("started_at").and_then(Value::as_i64),
        }),
        "task_complete" => Some(CodexItem::TaskComplete {
            turn_id,
            duration_ms: p.get("duration_ms").and_then(Value::as_u64),
            last_agent_message: s(p, "last_agent_message")
                .map(|m| clip(&m, MAX_MESSAGE_CHARS * WINDOW)),
        }),
        "item_completed" => {
            let item = p.get("item")?;
            match item.get("type")?.as_str()? {
                "CommandExecution" => Some(CodexItem::Command {
                    turn_id,
                    command: command_text(item.get("command").unwrap_or(&Value::Null)),
                    cwd: clip(&s(item, "cwd").unwrap_or_default(), MAX_PATH_CHARS),
                    exit_code: item.get("exit_code").and_then(Value::as_i64),
                    duration_ms: item.get("duration").and_then(duration_ms),
                    output_tail: s(item, "aggregated_output")
                        .filter(|o| !o.is_empty())
                        .map(|o| tail(&o, MAX_OUTPUT_TAIL_CHARS * WINDOW)),
                }),
                "FileChange" => Some(CodexItem::FileChange {
                    turn_id,
                    paths: change_paths(item.get("changes").unwrap_or(&Value::Null)),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Option<CodexItem> {
        parse_line(line.as_bytes())
    }

    #[test]
    fn task_started_and_complete() {
        assert_eq!(
            p(
                r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1","started_at":1791978621,"extra":1}}"#
            ),
            Some(CodexItem::TaskStarted {
                turn_id: "t1".into(),
                started_at: Some(1791978621)
            })
        );
        assert_eq!(
            p(
                r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","duration_ms":55061,"last_agent_message":"done"}}"#
            ),
            Some(CodexItem::TaskComplete {
                turn_id: "t1".into(),
                duration_ms: Some(55061),
                last_agent_message: Some("done".into())
            })
        );
    }

    #[test]
    fn command_execution() {
        let line = r#"{"type":"event_msg","payload":{"type":"item_completed","turn_id":"t1","item":{"type":"CommandExecution","command":["bash","-lc","cargo test\nmore"],"cwd":"/r","exit_code":101,"duration":{"secs":2,"nanos":500000000},"aggregated_output":"test result: FAILED"}}}"#;
        assert_eq!(
            p(line),
            Some(CodexItem::Command {
                turn_id: "t1".into(),
                command: "cargo test".into(),
                cwd: "/r".into(),
                exit_code: Some(101),
                duration_ms: Some(2500),
                output_tail: Some("test result: FAILED".into()),
            })
        );
    }

    #[test]
    fn file_change_map_and_list() {
        let m = r#"{"type":"event_msg","payload":{"type":"item_completed","turn_id":"t","item":{"type":"FileChange","changes":{"/r/a.rs":{"kind":"update"},"/r/b.rs":{}}}}}"#;
        assert_eq!(
            p(m),
            Some(CodexItem::FileChange {
                turn_id: "t".into(),
                paths: vec!["/r/a.rs".into(), "/r/b.rs".into()]
            })
        );
        let l = r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","changes":[{"path":"/r/c.rs"}]}}}"#;
        assert_eq!(
            p(l),
            Some(CodexItem::FileChange {
                turn_id: String::new(),
                paths: vec!["/r/c.rs".into()]
            })
        );
    }

    #[test]
    fn unknown_and_malformed_are_none() {
        assert_eq!(p(""), None);
        assert_eq!(p("not json"), None);
        assert_eq!(p("[1]"), None);
        assert_eq!(
            p(r#"{"type":"response_item","payload":{"type":"message"}}"#),
            None
        );
        assert_eq!(
            p(r#"{"type":"event_msg","payload":{"type":"token_count"}}"#),
            None
        );
        assert_eq!(
            p(
                r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning"}}}"#
            ),
            None
        );
        assert_eq!(parse_line(&[0xff, 0xfe, 0x00]), None);
        // Wrong types: tolerated.
        assert!(
            p(r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":5}}"#).is_some()
        );
    }
}
