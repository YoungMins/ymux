//! Process-tree scan that finds coding-agent CLIs running under each pane's
//! shell. The matcher and tree walk are pure (tested on Linux); only the
//! sysinfo refresh + emit loop is desktop-gated.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use uuid::Uuid;

/// One process, reduced to what the matcher needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Executable file stem: `claude` for `/usr/local/bin/claude` or `claude.exe`.
    pub exe_stem: String,
    pub argv: Vec<String>,
}

/// Executables that are an agent by name.
const AGENT_EXES: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "cursor-agent",
    "amp",
];

/// Script hosts whose argv identifies the agent package.
const SCRIPT_HOSTS: &[&str] = &["node", "bun"];

/// Package directories that identify an agent when a script host's *script*
/// lives inside them. Matched as whole path segments (leading and trailing
/// `/`) after `\` → `/` normalisation, so Windows `node_modules\@openai\codex\…`
/// matches but `claude-code-proxy\…` or `@openai/codex-tools/…` don't.
const SCRIPT_MARKERS: &[(&str, &str)] = &[
    ("/@anthropic-ai/claude-code/", "claude"),
    ("/@openai/codex/", "codex"),
    ("/@google/gemini-cli/", "gemini"),
];

/// Which agent, if any, a process is.
pub fn match_agent(exe_stem: &str, argv: &[String]) -> Option<&'static str> {
    let stem = exe_stem.to_ascii_lowercase();
    if let Some(name) = AGENT_EXES.iter().copied().find(|n| *n == stem) {
        return Some(name);
    }
    if !SCRIPT_HOSTS.contains(&stem.as_str()) {
        return None;
    }
    // Only the script argument counts: the first non-flag arg after the host
    // itself. Later args are the script's own input and may mention an agent
    // path without being one.
    let script = argv.iter().skip(1).find(|a| !a.starts_with('-'))?;
    let script = format!("/{}", script.replace('\\', "/").to_ascii_lowercase());
    SCRIPT_MARKERS
        .iter()
        .find(|(marker, _)| script.contains(marker))
        .map(|(_, kind)| *kind)
}

/// File stem of `exe`, or `name` minus a trailing `.exe` when the path is
/// unreadable (e.g. another user's process on Windows).
pub fn exe_stem(exe: Option<&Path>, name: &str) -> String {
    if let Some(stem) = exe.and_then(Path::file_stem) {
        return stem.to_string_lossy().into_owned();
    }
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
        .to_string()
}

/// Parent → children index over one process snapshot.
pub struct ProcTree<'a> {
    children: HashMap<u32, Vec<&'a ProcEntry>>,
}

impl<'a> ProcTree<'a> {
    pub fn new(procs: &'a [ProcEntry]) -> Self {
        let mut children: HashMap<u32, Vec<&'a ProcEntry>> = HashMap::new();
        for p in procs {
            if let Some(parent) = p.parent.filter(|pp| *pp != p.pid) {
                children.entry(parent).or_default().push(p);
            }
        }
        for list in children.values_mut() {
            list.sort_by_key(|p| p.pid);
        }
        Self { children }
    }

    /// Breadth-first from `root_pid` (exclusive): the agent closest to the
    /// shell wins. Cycle-safe against PID reuse.
    pub fn agent_under(&self, root_pid: u32) -> Option<&'static str> {
        let mut seen: HashSet<u32> = HashSet::from([root_pid]);
        let mut queue: VecDeque<u32> = VecDeque::from([root_pid]);
        while let Some(pid) = queue.pop_front() {
            for child in self.children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(child.pid) {
                    continue;
                }
                if let Some(kind) = match_agent(&child.exe_stem, &child.argv) {
                    return Some(kind);
                }
                queue.push_back(child.pid);
            }
        }
        None
    }
}

/// `pane id → agent kind` for every pane whose shell has an agent descendant.
pub fn scan_panes(shells: &HashMap<Uuid, u32>, procs: &[ProcEntry]) -> HashMap<Uuid, String> {
    let tree = ProcTree::new(procs);
    shells
        .iter()
        .filter_map(|(id, pid)| tree.agent_under(*pid).map(|k| (*id, k.to_string())))
        .collect()
}

/// How often the process tree is re-scanned.
#[cfg(feature = "desktop")]
const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Background thread: every [`SCAN_INTERVAL`], walk each pane's shell
/// descendants for agent CLIs, feed the registry, and emit `agents:changed`
/// when the snapshot moved. Only exe + argv are refreshed, each once per
/// process, so steady-state cost is one process-list enumeration.
#[cfg(feature = "desktop")]
pub fn start_agent_scan(app: tauri::AppHandle) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    use tauri::Manager;

    use crate::agents::SharedAgents;
    use crate::commands::{emit_agents_changed, AppState};

    std::thread::Builder::new()
        .name("ymux-agent-scan".into())
        .spawn(move || {
            let mut sys = System::new();
            let refresh = ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet);
            loop {
                std::thread::sleep(SCAN_INTERVAL);
                let shells = app.state::<AppState>().pty.pids_snapshot();
                let found = if shells.is_empty() {
                    HashMap::new()
                } else {
                    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
                    let procs: Vec<ProcEntry> = sys
                        .processes()
                        .values()
                        .map(|p| ProcEntry {
                            pid: p.pid().as_u32(),
                            parent: p.parent().map(|pp| pp.as_u32()),
                            exe_stem: exe_stem(p.exe(), &p.name().to_string_lossy()),
                            argv: p
                                .cmd()
                                .iter()
                                .map(|a| a.to_string_lossy().into_owned())
                                .collect(),
                        })
                        .collect();
                    scan_panes(&shells, &procs)
                };
                let live: HashSet<Uuid> = shells.keys().copied().collect();
                let agents = app.state::<SharedAgents>();
                let mut reg = agents.0.lock();
                if reg.apply_scan(&live, &found) {
                    // Under the lock, so this can't race a hook-driven emit
                    // out of order (see `commands::apply_agent_hook`).
                    emit_agents_changed(&app, &reg.snapshot());
                }
            }
        })
        .expect("spawn agent scan thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn proc(pid: u32, parent: Option<u32>, stem: &str, args: &[&str]) -> ProcEntry {
        ProcEntry {
            pid,
            parent,
            exe_stem: stem.into(),
            argv: argv(args),
        }
    }

    #[test]
    fn matcher_accepts_known_agent_executables() {
        for (stem, kind) in [
            ("claude", "claude"),
            ("Claude", "claude"),
            ("codex", "codex"),
            ("gemini", "gemini"),
            ("opencode", "opencode"),
            ("aider", "aider"),
            ("cursor-agent", "cursor-agent"),
            ("amp", "amp"),
        ] {
            assert_eq!(match_agent(stem, &[]), Some(kind), "{stem}");
        }
    }

    #[test]
    fn matcher_recognises_script_hosted_agents() {
        let win = argv(&[
            "node",
            "C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js",
        ]);
        assert_eq!(match_agent("node", &win), Some("claude"));
        assert_eq!(
            match_agent(
                "bun",
                &argv(&["bun", "/x/node_modules/@openai/codex/bin/codex.js"])
            ),
            Some("codex")
        );
        let gemini_win = argv(&[
            "node",
            "C:\\npm\\node_modules\\@google\\gemini-cli\\dist\\index.js",
        ]);
        assert_eq!(match_agent("node", &gemini_win), Some("gemini"));
    }

    #[test]
    fn matcher_rejects_everything_else() {
        assert_eq!(match_agent("pwsh", &[]), None);
        assert_eq!(match_agent("bash", &argv(&["bash", "claude"])), None);
        assert_eq!(match_agent("claudette", &[]), None);
        assert_eq!(match_agent("node", &argv(&["node", "server.js"])), None);
        // Package markers only count inside a script host.
        assert_eq!(
            match_agent("python", &argv(&["python", "@openai/codex"])),
            None
        );
    }

    #[test]
    fn matcher_accepts_real_npm_global_install_paths() {
        let win = argv(&[
            r"C:\Program Files\nodejs\node.exe",
            r"C:\Users\x\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js",
            "--resume",
        ]);
        assert_eq!(match_agent("node", &win), Some("claude"));
        let unix = argv(&[
            "node",
            "/usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        ]);
        assert_eq!(match_agent("node", &unix), Some("claude"));
        // Host flags before the script are skipped.
        let flagged = argv(&[
            "node",
            "--max-old-space-size=4096",
            "/home/x/.npm-global/lib/node_modules/@openai/codex/bin/codex.js",
        ]);
        assert_eq!(match_agent("node", &flagged), Some("codex"));
    }

    #[test]
    fn matcher_ignores_lookalike_scripts_and_non_script_args() {
        // A project that merely has `claude-code` in its name.
        let proxy = argv(&["node", r"D:\Git\claude-code-proxy\server.js"]);
        assert_eq!(match_agent("node", &proxy), None);
        // The package path only counts as the script, not as a later arg.
        let later = argv(&[
            "node",
            "server.js",
            "/n/node_modules/@anthropic-ai/claude-code/cli.js",
        ]);
        assert_eq!(match_agent("node", &later), None);
        // A package-name prefix is not the package.
        let prefix = argv(&["node", "/n/node_modules/@openai/codex-tools/x.js"]);
        assert_eq!(match_agent("node", &prefix), None);
    }

    #[test]
    fn exe_stem_prefers_the_path_and_falls_back_to_the_name() {
        assert_eq!(
            exe_stem(Some(Path::new("/usr/local/bin/node")), "ignored"),
            "node"
        );
        assert_eq!(exe_stem(None, "claude.exe"), "claude");
        assert_eq!(exe_stem(None, "codex"), "codex");
    }

    #[test]
    fn walk_finds_a_nested_agent_under_the_shell() {
        // shell(10) → cmd(11) → node claude-code(12)
        let procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(11, Some(10), "cmd", &[]),
            proc(
                12,
                Some(11),
                "node",
                &["node", "/n/@anthropic-ai/claude-code/cli.js"],
            ),
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), Some("claude"));
    }

    #[test]
    fn walk_ignores_the_root_and_processes_outside_the_subtree() {
        let procs = vec![
            proc(10, Some(1), "claude", &[]), // the "shell" itself
            proc(20, Some(1), "codex", &[]),  // sibling, not a descendant
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), None);
    }

    #[test]
    fn walk_prefers_the_agent_closest_to_the_shell() {
        let procs = vec![
            proc(10, Some(1), "zsh", &[]),
            proc(30, Some(10), "claude", &[]),
            proc(31, Some(30), "codex", &[]),
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), Some("claude"));
    }

    #[test]
    fn walk_survives_parent_cycles_from_pid_reuse() {
        let procs = vec![proc(10, Some(11), "sh", &[]), proc(11, Some(10), "sh", &[])];
        assert_eq!(ProcTree::new(&procs).agent_under(10), None);
    }

    #[test]
    fn scan_panes_maps_each_pane_to_its_agent() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let procs = vec![
            proc(10, Some(1), "bash", &[]),
            proc(11, Some(10), "gemini", &[]),
            proc(20, Some(1), "bash", &[]),
        ];
        let shells: HashMap<Uuid, u32> = [(a, 10), (b, 20)].into_iter().collect();
        let found = scan_panes(&shells, &procs);
        assert_eq!(found.get(&a).map(String::as_str), Some("gemini"));
        assert!(!found.contains_key(&b));
    }
}
