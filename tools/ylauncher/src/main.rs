//! `y` — the Claude Code hook relay bundled with ymux.
//!
//! It was once the launcher for the `y*` TUI tools; those are GUI panes now
//! and the launcher went with them. The binary keeps its name and location
//! because the absolute path to it is written into every agent-tracking
//! user's `~/.claude/settings.json` (`src-tauri/src/agent_hooks.rs`), so a
//! rename would strand those hooks on a missing executable.
//!
//! `y agent-hook …` is the only command. Anything else prints one usage line
//! on stderr and exits 2 — it never reads stdin or writes stdout.

mod agent_hook;

const USAGE: &str = "usage: y agent-hook <agent> [--ymux-agent-hook]  \
                     (Claude Code hook relay for ymux; not meant to be run by hand)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if is_agent_hook(&args) {
        agent_hook::run(&args[1..]);
        return;
    }
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// `y agent-hook …`, with any (or no) further arguments. Decided on the first
/// argument alone so an odd agent name still takes the silent, exit-0 path.
fn is_agent_hook(args: &[String]) -> bool {
    args.first().is_some_and(|a| a == "agent-hook")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn agent_hook_is_recognised_whatever_follows() {
        assert!(is_agent_hook(&args(&["agent-hook"])));
        assert!(is_agent_hook(&args(&[
            "agent-hook",
            "claude",
            "--ymux-agent-hook"
        ])));
        assert!(is_agent_hook(&args(&["agent-hook", "--weird", "x", "y"])));
    }

    #[test]
    fn everything_else_is_a_usage_error() {
        assert!(!is_agent_hook(&args(&[])));
        // The retired launcher subcommands.
        for sub in ["mon", "dir", "code", "git", "help", "--help", "--version"] {
            assert!(!is_agent_hook(&args(&[sub])), "{sub}");
        }
        assert!(!is_agent_hook(&args(&["Agent-Hook"])));
    }
}
