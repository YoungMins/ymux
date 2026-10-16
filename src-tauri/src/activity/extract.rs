//! Claude Code hook JSON -> a small, clipped [`ActivityRaw`].
//!
//! Field names follow the official hooks reference
//! (<https://code.claude.com/docs/en/hooks>): common fields `session_id`,
//! `transcript_path`, `cwd`, `hook_event_name`, `agent_id`; `UserPromptSubmit`
//! `prompt`; `PreToolUse`/`PostToolUse`/`PermissionRequest` `tool_name` +
//! `tool_input` (`file_path` for Write/Edit, `command` for Bash,
//! `notebook_path` for NotebookEdit, `edits[].file_path` for MultiEdit);
//! `PostToolUse` `tool_response` (Bash: `stdout`/`stderr`/`interrupted`; Write:
//! `filePath`); `Stop`/`StopFailure` `last_assistant_message`, and
//! `StopFailure` `error`/`error_details`.
//!
//! Everything is optional and tolerant: an unknown event is `None`, a missing
//! or mistyped field is skipped, nothing panics. File *contents* (`content`,
//! `old_string`, `new_string`) are never read, only paths. Strings are clipped
//! but not yet redacted; the worker redacts before disk (see `redact`).
//!
//! Free text is clipped to a *window* of [`WINDOW`] times its final limit
//! here, on the hot path. The worker redacts the window and only then clips to
//! the final `MAX_*` size, so a token that straddles the final limit is
//! redacted whole instead of being cut into an unrecognisable fragment.

use super::redact::{clip, first_line_clipped, tail};
use crate::agent_sessions::is_valid_session_id;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_PATHS: usize = 20;
pub const MAX_PATH_CHARS: usize = 300;
pub const MAX_CMD_CHARS: usize = 200;
pub const MAX_PROMPT_CHARS: usize = 300;
pub const MAX_MESSAGE_CHARS: usize = 500;
pub const MAX_OUTPUT_TAIL_CHARS: usize = 600;
/// Hot-path clip = this many times the final limit (see the module docs).
pub const WINDOW: usize = 2;
/// Longest `agent_id` kept.
pub const MAX_AGENT_ID_CHARS: usize = 128;
/// Longest `session_id` considered (the validator is stricter still).
pub const MAX_SESSION_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawKind {
    Prompt,
    ToolStart,
    ToolEnd,
    Permission,
    Stop,
    StopFailure,
}

/// One hook event, reduced to what the activity log needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRaw {
    pub kind: RawKind,
    pub session_id: String,
    pub cwd: String,
    pub transcript_path: String,
    pub agent_id: Option<String>,
    pub prompt: Option<String>,
    pub tool_name: Option<String>,
    pub paths: Vec<String>,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub interrupted: bool,
    pub output_tail: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn push_path(paths: &mut Vec<String>, p: Option<&str>) {
    let Some(p) = p.map(str::trim).filter(|p| !p.is_empty()) else {
        return;
    };
    if paths.len() < MAX_PATHS {
        let p = clip(p, MAX_PATH_CHARS);
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
}

/// Paths a tool call touches, from its `tool_input` (and, for a few tools,
/// its `tool_response`). Never reads file contents.
fn tool_paths(input: Option<&Value>, response: Option<&Value>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(input) = input {
        for key in ["file_path", "notebook_path", "path"] {
            push_path(&mut paths, str_at(input, key));
        }
        if let Some(edits) = input.get("edits").and_then(Value::as_array) {
            for e in edits {
                push_path(&mut paths, str_at(e, "file_path"));
            }
        }
    }
    if let Some(resp) = response {
        push_path(&mut paths, str_at(resp, "filePath"));
    }
    paths
}

/// Exit code and output tail from a Bash `tool_response`. The docs show
/// `stdout`/`stderr`/`interrupted`; an exit code field is read when present
/// under any of its common spellings.
fn bash_result(resp: &Value) -> (Option<i64>, bool, Option<String>) {
    if let Some(s) = resp.as_str() {
        return (None, false, Some(tail(s, MAX_OUTPUT_TAIL_CHARS * WINDOW)));
    }
    let exit = ["exit_code", "exitCode", "returncode", "code"]
        .iter()
        .find_map(|k| resp.get(*k).and_then(Value::as_i64));
    let interrupted = resp
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Each stream is tailed on its own first: a huge stdout must neither be
    // copied whole nor push a short stderr out of the window.
    let mut out = String::new();
    for k in ["stdout", "stderr"] {
        if let Some(s) = str_at(resp, k).filter(|s| !s.is_empty()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&tail(s, MAX_OUTPUT_TAIL_CHARS * WINDOW));
        }
    }
    let out = (!out.is_empty()).then(|| tail(&out, MAX_OUTPUT_TAIL_CHARS * WINDOW));
    (exit, interrupted, out)
}

/// `None` for any event the log does not record (or a non-object body).
pub fn from_hook(body: &Value) -> Option<ActivityRaw> {
    if !body.is_object() {
        return None;
    }
    let event = str_at(body, "hook_event_name").or_else(|| str_at(body, "event"))?;
    let kind = match event {
        "UserPromptSubmit" => RawKind::Prompt,
        "PreToolUse" => RawKind::ToolStart,
        "PostToolUse" => RawKind::ToolEnd,
        "PermissionRequest" => RawKind::Permission,
        "Stop" => RawKind::Stop,
        "StopFailure" => RawKind::StopFailure,
        _ => return None,
    };
    let mut raw = ActivityRaw {
        kind,
        session_id: str_at(body, "session_id")
            .filter(|id| id.len() <= MAX_SESSION_ID_LEN && is_valid_session_id(id))
            .unwrap_or("")
            .to_owned(),
        cwd: clip(str_at(body, "cwd").unwrap_or(""), MAX_PATH_CHARS),
        transcript_path: clip(
            str_at(body, "transcript_path").unwrap_or(""),
            MAX_PATH_CHARS,
        ),
        agent_id: str_at(body, "agent_id").map(|a| clip(a, MAX_AGENT_ID_CHARS)),
        prompt: None,
        tool_name: None,
        paths: Vec::new(),
        command: None,
        exit_code: None,
        interrupted: false,
        output_tail: None,
        message: None,
        error: None,
    };
    match kind {
        RawKind::Prompt => {
            raw.prompt = str_at(body, "prompt").map(|p| clip(p, MAX_PROMPT_CHARS * WINDOW));
        }
        RawKind::ToolStart | RawKind::ToolEnd | RawKind::Permission => {
            raw.tool_name = str_at(body, "tool_name").map(|t| clip(t, 80));
            let input = body.get("tool_input");
            let response = body.get("tool_response");
            raw.paths = tool_paths(input, response);
            raw.command = input
                .and_then(|i| str_at(i, "command"))
                .map(|c| first_line_clipped(c, MAX_CMD_CHARS * WINDOW));
            if kind == RawKind::ToolEnd && raw.tool_name.as_deref() == Some("Bash") {
                if let Some(resp) = response {
                    (raw.exit_code, raw.interrupted, raw.output_tail) = bash_result(resp);
                }
            }
        }
        RawKind::Stop | RawKind::StopFailure => {
            raw.message =
                str_at(body, "last_assistant_message").map(|m| clip(m, MAX_MESSAGE_CHARS * WINDOW));
            if kind == RawKind::StopFailure {
                raw.error = str_at(body, "error").map(|e| clip(e, 80));
            }
        }
    }
    Some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompt_is_clipped() {
        let long = "x".repeat(1000);
        let raw = from_hook(&json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "a1", "cwd": "/r", "prompt": long,
            "transcript_path": "/t.jsonl",
        }))
        .unwrap();
        assert_eq!(raw.kind, RawKind::Prompt);
        assert_eq!(raw.session_id, "a1");
        // The hot path keeps a window; the worker does the final clip.
        assert_eq!(
            raw.prompt.unwrap().chars().count(),
            MAX_PROMPT_CHARS * WINDOW
        );
    }

    #[test]
    fn post_tool_write_collects_path_not_content() {
        let raw = from_hook(&json!({
            "hook_event_name": "PostToolUse", "session_id": "a1", "cwd": "/r",
            "tool_name": "Write",
            "tool_input": {"file_path": "/r/a.txt", "content": "SECRET BODY"},
            "tool_response": {"filePath": "/r/a.txt", "type": "create"},
        }))
        .unwrap();
        assert_eq!(raw.kind, RawKind::ToolEnd);
        assert_eq!(raw.paths, vec!["/r/a.txt"]);
        assert!(!serde_json::to_string(&raw).unwrap().contains("SECRET BODY"));
    }

    #[test]
    fn multiedit_and_notebook_paths_cap_at_twenty() {
        let edits: Vec<Value> = (0..30)
            .map(|i| json!({"file_path": format!("/r/f{i}.rs"), "old_string": "a"}))
            .collect();
        let raw = from_hook(&json!({
            "hook_event_name": "PreToolUse", "tool_name": "MultiEdit",
            "tool_input": {"file_path": "/r/main.rs", "edits": edits},
        }))
        .unwrap();
        assert_eq!(raw.paths.len(), MAX_PATHS);
        assert_eq!(raw.paths[0], "/r/main.rs");
        let nb = from_hook(&json!({
            "hook_event_name": "PreToolUse", "tool_name": "NotebookEdit",
            "tool_input": {"notebook_path": "/r/n.ipynb"},
        }))
        .unwrap();
        assert_eq!(nb.paths, vec!["/r/n.ipynb"]);
    }

    #[test]
    fn bash_command_first_line_and_result() {
        let raw = from_hook(&json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": format!("cargo test\necho {}", "y".repeat(400))},
            "tool_response": {"stdout": "test result: ok. 3 passed", "stderr": "",
                              "interrupted": false, "exit_code": 0},
        }))
        .unwrap();
        assert_eq!(raw.command.as_deref(), Some("cargo test"));
        assert_eq!(raw.exit_code, Some(0));
        assert_eq!(
            raw.output_tail.as_deref(),
            Some("test result: ok. 3 passed")
        );
        let long = from_hook(&json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "tool_input": {"command": "z".repeat(500)},
        }))
        .unwrap();
        assert_eq!(
            long.command.unwrap().chars().count(),
            MAX_CMD_CHARS * WINDOW
        );
    }

    #[test]
    fn bash_string_response_and_missing_exit() {
        let raw = from_hook(&json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": "ls"}, "tool_response": "a\nb",
        }))
        .unwrap();
        assert_eq!(raw.exit_code, None);
        assert_eq!(raw.output_tail.as_deref(), Some("a\nb"));
    }

    #[test]
    fn stop_and_stop_failure() {
        let stop = from_hook(&json!({
            "hook_event_name": "Stop", "session_id": "a",
            "last_assistant_message": "done", "stop_hook_active": false,
        }))
        .unwrap();
        assert_eq!(stop.kind, RawKind::Stop);
        assert_eq!(stop.message.as_deref(), Some("done"));
        let fail = from_hook(&json!({
            "hook_event_name": "StopFailure", "error": "rate_limit",
            "error_details": "429", "last_assistant_message": "API Error",
        }))
        .unwrap();
        assert_eq!(fail.kind, RawKind::StopFailure);
        assert_eq!(fail.error.as_deref(), Some("rate_limit"));
    }

    #[test]
    fn permission_request_keeps_tool_and_command() {
        let raw = from_hook(&json!({
            "hook_event_name": "PermissionRequest", "tool_name": "Bash",
            "tool_input": {"command": "rm -rf node_modules"},
            "permission_suggestions": [{"type": "addRules"}],
        }))
        .unwrap();
        assert_eq!(raw.kind, RawKind::Permission);
        assert_eq!(raw.command.as_deref(), Some("rm -rf node_modules"));
    }

    #[test]
    fn unknown_or_malformed_input_is_none_or_tolerated() {
        assert!(from_hook(&json!(null)).is_none());
        assert!(from_hook(&json!([1, 2])).is_none());
        assert!(from_hook(&json!({})).is_none());
        assert!(from_hook(&json!({"hook_event_name": "SubagentStart"})).is_none());
        // Wrong types everywhere: still no panic, fields just drop out.
        let raw = from_hook(&json!({
            "hook_event_name": "PostToolUse", "session_id": 7, "cwd": [],
            "tool_name": {}, "tool_input": "oops", "tool_response": 5,
        }))
        .unwrap();
        assert_eq!(raw.session_id, "");
        assert!(raw.paths.is_empty());
        assert_eq!(raw.tool_name, None);
    }

    #[test]
    fn a_huge_session_or_agent_id_is_dropped_or_clipped() {
        let huge = "a".repeat(8 * 1024 * 1024);
        let raw = from_hook(&json!({
            "hook_event_name": "Stop", "session_id": huge, "agent_id": huge,
        }))
        .unwrap();
        assert_eq!(raw.session_id, "");
        assert_eq!(raw.agent_id.unwrap().chars().count(), MAX_AGENT_ID_CHARS);
        // Not an id the resume machinery would accept: dropped too.
        let raw = from_hook(&json!({
            "hook_event_name": "Stop", "session_id": "../../etc/passwd",
        }))
        .unwrap();
        assert_eq!(raw.session_id, "");
        let ok = from_hook(&json!({
            "hook_event_name": "Stop",
            "session_id": "01a00000-0000-0000-0000-0000000000aa",
        }))
        .unwrap();
        assert_eq!(ok.session_id, "01a00000-0000-0000-0000-0000000000aa");
    }

    #[test]
    fn stdout_and_stderr_are_tailed_separately() {
        let raw = from_hook(&json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": {"command": "x"},
            "tool_response": {"stdout": "o".repeat(2_000_000), "stderr": "BOOM"},
        }))
        .unwrap();
        let out = raw.output_tail.unwrap();
        assert!(out.ends_with("\nBOOM"), "stderr must survive");
        assert!(out.chars().count() <= MAX_OUTPUT_TAIL_CHARS * WINDOW);
    }
}
