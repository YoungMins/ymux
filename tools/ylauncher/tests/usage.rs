//! `y` is only the hook relay now: anything but `agent-hook` is a usage
//! error — one line on stderr, nothing on stdout, exit 2, stdin untouched.

use std::process::{Command, Output, Stdio};

fn run_y(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_y"))
        .args(args)
        .env_remove("YMUX_IPC")
        .env_remove("YMUX_PANE_ID")
        // Null stdin: a usage error must not block waiting for input.
        .stdin(Stdio::null())
        .output()
        .expect("run y")
}

fn assert_usage_error(out: &Output) {
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("usage: y agent-hook"), "{err}");
    assert_eq!(err.trim_end().lines().count(), 1, "{err}");
}

#[test]
fn no_arguments_is_a_usage_error() {
    assert_usage_error(&run_y(&[]));
}

#[test]
fn a_retired_launcher_subcommand_is_a_usage_error() {
    assert_usage_error(&run_y(&["mon"]));
    assert_usage_error(&run_y(&["code", "main.rs"]));
    assert_usage_error(&run_y(&["--version"]));
}

#[test]
fn agent_hook_with_an_odd_agent_still_exits_zero_silently() {
    let out = run_y(&["agent-hook", "--only-flags"]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
}
