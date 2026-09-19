mod agent_hook;
use std::path::PathBuf;
use std::process::Command;

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "ymon",
        description: "System monitor (htop/btop-style TUI)",
    },
    ToolDef {
        name: "ydir",
        description: "Dual-pane file manager (mdir-style TUI)",
    },
    ToolDef {
        name: "ycode",
        description: "Code editor (VS Code-style TUI)",
    },
    ToolDef {
        name: "ygit",
        description: "Git log & branch viewer TUI",
    },
];

struct ToolDef {
    name: &'static str,
    description: &'static str,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return;
    }

    if args[0] == "--version" || args[0] == "-V" {
        println!("y (ylauncher) {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // Hook relay for the ymux agent tree. Must be matched before the
    // `y<tool>` lookup below, which would otherwise look for `yagent-hook`.
    if args[0] == "agent-hook" {
        agent_hook::run(&args[1..]);
        return;
    }

    let tool_name = format!("y{}", args[0]);
    let tool_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    if let Some(path) = find_tool(&tool_name) {
        let status = Command::new(&path).args(&tool_args).status();

        match status {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("y: failed to run {}: {}", tool_name, e);
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("y: tool '{}' not found in PATH", tool_name);
        eprintln!();
        print_help();
        std::process::exit(1);
    }
}

fn print_help() {
    println!("y — ymux tool launcher");
    println!();
    println!("USAGE:");
    println!("    y <tool> [args...]    Launch a y* tool");
    println!("    y help               Show this help");
    println!("    y --version          Show version");
    println!();
    println!("AVAILABLE TOOLS:");
    println!();

    let installed = discover_tools();

    for tool in TOOLS {
        let status = if installed.iter().any(|t| t == tool.name) {
            "\x1b[32m✓\x1b[0m"
        } else {
            "\x1b[31m✗\x1b[0m"
        };
        println!("  {} {:8} {}", status, tool.name, tool.description);
    }

    println!();
    println!("EXAMPLES:");
    println!("    y mon           Launch system monitor");
    println!("    y dir           Launch file manager");
    println!("    y code file.rs  Open file in editor");
    println!("    y git           Browse git log and branches");
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };

    // Check PATH
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(&exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // Check next to the current executable
    if let Ok(self_path) = std::env::current_exe() {
        if let Some(dir) = self_path.parent() {
            let candidate = dir.join(&exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn discover_tools() -> Vec<String> {
    TOOLS
        .iter()
        .filter(|t| find_tool(t.name).is_some())
        .map(|t| t.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_tool_finds_common_binaries() {
        // On any system, at least one common tool should be findable
        // (this verifies PATH scanning logic works)
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(find_tool(shell).is_some(), "should find {} in PATH", shell);
    }

    #[test]
    fn find_tool_returns_none_for_nonexistent() {
        assert!(find_tool("nonexistent_tool_xyz_123").is_none());
    }

    #[test]
    fn discover_tools_returns_vec() {
        let tools = discover_tools();
        // In test env, y* tools likely aren't built yet
        assert!(tools.len() <= TOOLS.len());
    }

    #[test]
    fn tool_defs_have_content() {
        for tool in TOOLS {
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty());
            assert!(tool.name.starts_with('y'));
        }
    }
}
