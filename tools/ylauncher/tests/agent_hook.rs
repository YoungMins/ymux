//! Binary-level checks: `y agent-hook` must be invisible to Claude Code —
//! exit 0, no stdout — whether or not it runs inside ymux.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use yipc::{IpcMessage, IpcServer, MessageHandler};

const INPUT: &str = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;

fn run_hook(envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_y"));
    cmd.args(["agent-hook", "claude", "--ymux-agent-hook"])
        .env_remove("YMUX_IPC")
        .env_remove("YMUX_PANE_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn y");
    // Outside ymux the hook exits without reading stdin; ignore EPIPE.
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(INPUT.as_bytes());
    child.wait_with_output().expect("wait")
}

#[test]
fn agent_hook_is_a_silent_noop_outside_ymux() {
    let out = run_hook(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[test]
fn agent_hook_exits_zero_when_ymux_is_unreachable() {
    let dead = if cfg!(windows) {
        "tcp:127.0.0.1:1"
    } else {
        "/nonexistent/ymux.sock"
    };
    let out = run_hook(&[("YMUX_IPC", dead), ("YMUX_PANE_ID", "p1")]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[test]
fn agent_hook_relays_the_event_to_ymux() {
    let (tx, rx) = mpsc::channel();
    let handler: MessageHandler = Box::new(move |msg, writer| {
        tx.send(msg).ok();
        let _ = writer.write_all(IpcMessage::Ack.to_line().unwrap().as_bytes());
        let _ = writer.flush();
    });
    let server = IpcServer::start(handler).expect("server");
    std::thread::sleep(Duration::from_millis(50));
    let out = run_hook(&[("YMUX_IPC", server.address()), ("YMUX_PANE_ID", "pane-1")]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    match rx.recv_timeout(Duration::from_secs(2)).expect("event") {
        IpcMessage::Event { kind, payload } => {
            assert_eq!(kind, "agent-hook");
            assert_eq!(payload["pane_id"], "pane-1");
            assert_eq!(payload["agent"], "claude");
            assert_eq!(payload["event"], "PreToolUse");
            assert_eq!(payload["tool_name"], "Bash");
        }
        other => panic!("unexpected message: {other:?}"),
    }
    server.shutdown();
}
