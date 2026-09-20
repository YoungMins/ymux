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

    /// The deepest descendant of `root_pid` (exclusive): a shell that reached
    /// `ycode` through a wrapper reports `ycode`, not the wrapper. Breadth
    /// first, so a tie at the same depth resolves to the lowest pid (children
    /// are pid-sorted in `new`). Cycle-safe against PID reuse, like
    /// `agent_under`.
    pub fn deepest_under(&self, root_pid: u32) -> Option<&'a ProcEntry> {
        let mut seen: HashSet<u32> = HashSet::from([root_pid]);
        let mut queue: VecDeque<(u32, u32)> = VecDeque::from([(root_pid, 0u32)]);
        let mut best: Option<(u32, &'a ProcEntry)> = None;
        while let Some((pid, depth)) = queue.pop_front() {
            for child in self.children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(child.pid) {
                    continue;
                }
                // `Option::is_none_or` would read better but is stable only
                // since 1.82; this crate's clippy MSRV is 1.77.
                if best.map_or(true, |(d, _)| depth + 1 > d) {
                    best = Some((depth + 1, child));
                }
                queue.push_back((child.pid, depth + 1));
            }
        }
        best.map(|(_, p)| p)
    }

    /// The tab label for the shell `root_pid`, or `None` for a bare shell.
    ///
    /// A *known* program wins over depth: a working agent constantly spawns
    /// and reaps helpers (`rg`, `node`, `bash`), so labelling the deepest
    /// descendant made a busy `claude` flicker through its children's names.
    /// Only when nothing under the shell is recognised does the old
    /// deepest-descendant rule apply, which is what still names a `vim` under
    /// a wrapper.
    pub fn label_under(&self, root_pid: u32) -> Option<String> {
        self.known_label_under(root_pid)
            .or_else(|| self.deepest_under(root_pid).map(proc_label))
    }

    /// Breadth-first from `root_pid` (exclusive) for the nearest process we
    /// label by name: a known agent (labelled by its kind, so `node` running
    /// the claude-code CLI reads `claude`) or one of [`FILE_ARG_EXES`], whose
    /// own label carries the file it opened. Nearest-wins and cycle-safety
    /// match [`Self::agent_under`].
    fn known_label_under(&self, root_pid: u32) -> Option<String> {
        let mut seen: HashSet<u32> = HashSet::from([root_pid]);
        let mut queue: VecDeque<u32> = VecDeque::from([root_pid]);
        while let Some(pid) = queue.pop_front() {
            for child in self.children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(child.pid) {
                    continue;
                }
                if let Some(kind) = match_agent(&child.exe_stem, &child.argv) {
                    return Some(kind.to_string());
                }
                if FILE_ARG_EXES.contains(&child.exe_stem.to_ascii_lowercase().as_str()) {
                    return Some(proc_label(child));
                }
                queue.push_back(child.pid);
            }
        }
        None
    }
}

/// Executables whose first path-like argument belongs in the label, because
/// the file *is* what the pane is showing.
const FILE_ARG_EXES: &[&str] = &["ycode"];

/// How a running process is labelled on a tab: its executable stem, plus the
/// file it opened for the editors in [`FILE_ARG_EXES`] (`ycode: main.rs`).
pub fn proc_label(entry: &ProcEntry) -> String {
    let stem = entry.exe_stem.to_ascii_lowercase();
    if !FILE_ARG_EXES.contains(&stem.as_str()) {
        return stem;
    }
    // First non-flag argument after the program itself, reduced to its file
    // name so a long absolute path can't blow up the strip.
    let file = entry
        .argv
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .and_then(|a| a.replace('\\', "/").rsplit('/').next().map(str::to_string))
        .filter(|f| !f.is_empty());
    match file {
        Some(f) => format!("{stem}: {f}"),
        None => stem,
    }
}

/// `pane id -> label` for every pane whose shell has at least one descendant.
/// Panes running a bare shell are absent, and the frontend falls back to the
/// shell name for those (see `src/terminal/tabLabel.ts`).
pub fn scan_labels(shells: &HashMap<Uuid, u32>, procs: &[ProcEntry]) -> HashMap<Uuid, String> {
    let tree = ProcTree::new(procs);
    shells
        .iter()
        .filter_map(|(id, pid)| tree.label_under(*pid).map(|label| (*id, label)))
        .collect()
}

/// Tauri-managed handle holding the latest label snapshot, so `get_pane_labels`
/// can seed a freshly mounted frontend between scans. Not desktop-gated: it is
/// a plain map behind a mutex (CLAUDE.md rule 1).
#[derive(Default)]
pub struct SharedLabels(pub parking_lot::Mutex<HashMap<Uuid, String>>);

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
                let (found, labels) = if shells.is_empty() {
                    (HashMap::new(), HashMap::new())
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
                    (scan_panes(&shells, &procs), scan_labels(&shells, &procs))
                };
                {
                    // Tab labels are separate state from the agent registry and
                    // are emitted only on a change, so an idle app is silent.
                    let shared = app.state::<SharedLabels>();
                    let mut current = shared.0.lock();
                    if *current != labels {
                        *current = labels;
                        crate::commands::emit_pane_labels(&app, &current);
                    }
                }
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

    #[test]
    fn proc_label_is_the_executable_stem() {
        assert_eq!(proc_label(&proc(1, None, "claude", &["claude"])), "claude");
        assert_eq!(
            proc_label(&proc(1, None, "pwsh", &["pwsh", "-NoLogo"])),
            "pwsh"
        );
    }

    #[test]
    fn proc_label_reports_ycodes_file_argument() {
        assert_eq!(
            proc_label(&proc(1, None, "ycode", &["ycode", "/home/x/src/main.rs"])),
            "ycode: main.rs"
        );
        assert_eq!(
            proc_label(&proc(
                1,
                None,
                "ycode",
                &["ycode", r"D:\Git\ymux\src\app.ts"]
            )),
            "ycode: app.ts"
        );
        // Flags before the path are skipped; a bare `ycode` stays bare.
        assert_eq!(
            proc_label(&proc(
                1,
                None,
                "ycode",
                &["ycode", "--readonly", "notes.md"]
            )),
            "ycode: notes.md"
        );
        assert_eq!(proc_label(&proc(1, None, "ycode", &["ycode"])), "ycode");
    }

    #[test]
    fn deepest_under_walks_past_wrappers_to_the_leaf() {
        // shell(10) -> cmd(11) -> node(12): the innermost process is the one
        // the user is looking at, so that is what the tab is called.
        let procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(11, Some(10), "cmd", &[]),
            proc(12, Some(11), "node", &["node", "server.js"]),
        ];
        assert_eq!(
            ProcTree::new(&procs).deepest_under(10).map(|p| p.pid),
            Some(12)
        );
    }

    #[test]
    fn deepest_under_is_none_for_a_bare_shell_and_ignores_siblings() {
        let procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(20, Some(1), "claude", &[]),
        ];
        assert_eq!(ProcTree::new(&procs).deepest_under(10), None);
    }

    #[test]
    fn deepest_under_survives_parent_cycles_from_pid_reuse() {
        let procs = vec![proc(10, Some(11), "sh", &[]), proc(11, Some(10), "sh", &[])];
        assert_eq!(
            ProcTree::new(&procs).deepest_under(10).map(|p| p.pid),
            Some(11)
        );
    }

    #[test]
    fn scan_labels_covers_every_pane_with_a_child_process() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let procs = vec![
            proc(10, Some(1), "bash", &[]),
            proc(11, Some(10), "ycode", &["ycode", "/w/README.md"]),
            proc(20, Some(1), "pwsh", &[]),
            proc(30, Some(1), "zsh", &[]),
            proc(31, Some(30), "claude", &["claude"]),
        ];
        let shells: HashMap<Uuid, u32> = [(a, 10), (b, 20), (c, 30)].into_iter().collect();
        let found = scan_labels(&shells, &procs);
        assert_eq!(found.get(&a).map(String::as_str), Some("ycode: README.md"));
        assert!(!found.contains_key(&b), "a bare shell reports no label");
        assert_eq!(found.get(&c).map(String::as_str), Some("claude"));
    }

    #[test]
    fn label_prefers_the_known_agent_over_its_busy_children() {
        // A working `claude` constantly spawns and reaps helpers. Labelling
        // the deepest descendant would flicker `rg` / `node` / `bash` onto
        // the tab; the agent itself is what the pane is running.
        let procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(11, Some(10), "claude", &["claude"]),
            proc(12, Some(11), "rg", &["rg", "--json", "foo"]),
            proc(13, Some(11), "node", &["node", "build.js"]),
            proc(14, Some(12), "bash", &["bash", "-c", "true"]),
        ];
        let id = Uuid::new_v4();
        let shells: HashMap<Uuid, u32> = [(id, 10)].into_iter().collect();
        assert_eq!(
            scan_labels(&shells, &procs).get(&id).map(String::as_str),
            Some("claude"),
        );
    }

    #[test]
    fn label_keeps_the_editor_file_when_the_editor_has_children() {
        // `ycode: <file>` is the whole point of FILE_ARG_EXES, and ycode
        // shelling out (git, a formatter) must not rename the tab.
        let procs = vec![
            proc(10, Some(1), "bash", &[]),
            proc(11, Some(10), "ycode", &["ycode", "/w/src/main.rs"]),
            proc(12, Some(11), "git", &["git", "diff"]),
        ];
        let id = Uuid::new_v4();
        let shells: HashMap<Uuid, u32> = [(id, 10)].into_iter().collect();
        assert_eq!(
            scan_labels(&shells, &procs).get(&id).map(String::as_str),
            Some("ycode: main.rs"),
        );
    }

    #[test]
    fn label_falls_back_to_the_deepest_descendant_without_a_known_program() {
        // Nothing here is an agent or an editor we label specially, so the
        // old deepest-wins rule still names the tab after what the user is
        // actually looking at.
        let procs = vec![
            proc(10, Some(1), "bash", &[]),
            proc(11, Some(10), "vim", &["vim", "notes.txt"]),
        ];
        let id = Uuid::new_v4();
        let shells: HashMap<Uuid, u32> = [(id, 10)].into_iter().collect();
        assert_eq!(
            scan_labels(&shells, &procs).get(&id).map(String::as_str),
            Some("vim"),
        );
    }
}
