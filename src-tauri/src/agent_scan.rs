//! Process-tree scan that finds coding-agent CLIs running under each pane's
//! shell. The matcher and tree walk are pure (tested on Linux); only the
//! sysinfo refresh + emit loop is desktop-gated.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
    /// Process start, whole seconds since the Unix epoch (0 when unknown).
    pub start_secs: u64,
    /// Working directory, when the OS let us read it.
    pub cwd: Option<String>,
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

/// The agent named exactly by `path`'s basename, ignoring case and a
/// trailing `.exe` / `.cmd`. `claude-helper.js` or `claudette` are not agents.
fn agent_by_basename(path: &str) -> Option<&'static str> {
    let base = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".cmd"))
        .unwrap_or(&base);
    AGENT_EXES.iter().copied().find(|n| *n == base)
}

/// A file stem made of digits and dots, like the native Claude binary's
/// `2.1` (from `versions/2.1.30`).
fn is_version_stem(stem: &str) -> bool {
    stem.bytes().any(|b| b.is_ascii_digit())
        && stem.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Which agent, if any, a process is.
pub fn match_agent(exe_stem: &str, argv: &[String]) -> Option<&'static str> {
    let stem = exe_stem.to_ascii_lowercase();
    if let Some(name) = AGENT_EXES.iter().copied().find(|n| *n == stem) {
        return Some(name);
    }
    // argv[0] names the agent even when the executable file doesn't: the
    // native Claude binary lives at a version-named path
    // (`~/.local/share/claude/versions/2.1.30`) and sets `process.title` to
    // `claude`, which on macOS overwrites the argv block the scan reads.
    // Trusted only in that shape — a version-number stem, or `claude`
    // itself — so an unrelated program's argv[0] can't claim to be an agent.
    if let Some(name) = argv.first().and_then(|a0| agent_by_basename(a0)) {
        if name == "claude" || is_version_stem(&stem) {
            return Some(name);
        }
    }
    if !SCRIPT_HOSTS.contains(&stem.as_str()) {
        return None;
    }
    // Only the script argument counts: the first non-flag arg after the host
    // itself. Later args are the script's own input and may mention an agent
    // path without being one.
    let script = argv.iter().skip(1).find(|a| !a.starts_with('-'))?;
    let normalised = format!("/{}", script.replace('\\', "/").to_ascii_lowercase());
    SCRIPT_MARKERS
        .iter()
        .find(|(marker, _)| normalised.contains(marker))
        .map(|(_, kind)| *kind)
        // A shebang launch passes the npm bin *symlink*, not the package
        // path: `node /opt/homebrew/bin/claude`. Its basename is the agent.
        .or_else(|| agent_by_basename(script))
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
    by_pid: HashMap<u32, &'a ProcEntry>,
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
        let by_pid = procs.iter().map(|p| (p.pid, p)).collect();
        Self { children, by_pid }
    }

    /// Breadth-first from `root_pid` (exclusive): the agent closest to the
    /// shell wins. Cycle-safe against PID reuse.
    pub fn agent_under(&self, root_pid: u32) -> Option<&'static str> {
        self.agent_proc_under(root_pid).map(|(kind, _)| kind)
    }

    /// [`ProcTree::agent_under`], with the process itself.
    pub fn agent_proc_under(&self, root_pid: u32) -> Option<(&'static str, &'a ProcEntry)> {
        let mut seen: HashSet<u32> = HashSet::from([root_pid]);
        let mut queue: VecDeque<u32> = VecDeque::from([root_pid]);
        while let Some(pid) = queue.pop_front() {
            for child in self.children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(child.pid) {
                    continue;
                }
                if let Some(kind) = match_agent(&child.exe_stem, &child.argv) {
                    return Some((kind, child));
                }
                queue.push_back(child.pid);
            }
        }
        None
    }

    /// `pid` and every descendant of it. Cycle-safe against PID reuse.
    pub fn subtree(&self, pid: u32) -> BTreeSet<u32> {
        let mut out = BTreeSet::from([pid]);
        let mut queue = VecDeque::from([pid]);
        while let Some(p) = queue.pop_front() {
            for child in self.children.get(&p).map(Vec::as_slice).unwrap_or(&[]) {
                if out.insert(child.pid) {
                    queue.push_back(child.pid);
                }
            }
        }
        out
    }

    /// Every running agent on the machine, once: an agent process whose
    /// *direct* parent is itself an agent (a node wrapper's native child, a
    /// daemon's pty host) is the same agent, not a second one.
    ///
    /// Only the direct parent counts. An agent further up the tree — ymux
    /// itself started from an agent's shell, say — does not make the agents
    /// in ymux's panes part of it; they are separate conversations.
    pub fn root_agents(&self) -> Vec<(&'static str, &'a ProcEntry)> {
        let mut out: Vec<(&'static str, &'a ProcEntry)> = self
            .by_pid
            .values()
            .filter_map(|p| match_agent(&p.exe_stem, &p.argv).map(|k| (k, *p)))
            .filter(|(_, p)| !self.parent_is_agent(p))
            .collect();
        out.sort_by_key(|(_, p)| p.pid);
        out
    }

    fn parent_is_agent(&self, p: &ProcEntry) -> bool {
        p.parent
            .filter(|pp| *pp != p.pid)
            .and_then(|pp| self.by_pid.get(&pp))
            .is_some_and(|parent| match_agent(&parent.exe_stem, &parent.argv).is_some())
    }

    /// The deepest descendant of `root_pid` (exclusive): a shell that reached
    /// `vim` through a wrapper reports `vim`, not the wrapper. Breadth
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
    /// A known agent wins over depth, labelled by its kind (so `node` running
    /// the claude-code CLI reads `claude`): a working agent constantly spawns
    /// and reaps helpers (`rg`, `node`, `bash`), so labelling the deepest
    /// descendant made a busy `claude` flicker through its children's names.
    /// Only when no agent is under the shell does the old deepest-descendant
    /// rule apply, which is what still names a `vim` under a wrapper.
    pub fn label_under(&self, root_pid: u32) -> Option<String> {
        self.agent_under(root_pid)
            .map(str::to_string)
            .or_else(|| self.deepest_under(root_pid).map(proc_label))
    }
}

/// How a running process is labelled on a tab: its lower-cased executable
/// stem.
pub fn proc_label(entry: &ProcEntry) -> String {
    entry.exe_stem.to_ascii_lowercase()
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

/// The agent process found under one pane's shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAgent {
    pub kind: String,
    pub process: crate::agent_binding::PaneProcess,
}

/// `pane id → its agent process` for every pane whose shell has one: the same
/// agent [`scan_panes`] reports, with what `agent_binding` needs to tell which
/// conversation that process is running.
pub fn scan_pane_agents(
    shells: &HashMap<Uuid, u32>,
    procs: &[ProcEntry],
) -> HashMap<Uuid, PaneAgent> {
    let tree = ProcTree::new(procs);
    shells
        .iter()
        .filter_map(|(id, pid)| {
            let (kind, p) = tree.agent_proc_under(*pid)?;
            Some((
                *id,
                PaneAgent {
                    kind: kind.to_string(),
                    process: crate::agent_binding::PaneProcess {
                        pid: p.pid,
                        start_secs: p.start_secs,
                        argv: p.argv.clone(),
                        subtree: tree.subtree(p.pid),
                    },
                },
            ))
        })
        .collect()
}

/// Every running agent ymux knows how to resume, as `agent_binding` weighs a
/// competing author of a transcript. `known_id` is filled from the process's
/// own argv and, for Claude, from `pid_file_id` (its per-process file).
pub fn other_agents(
    procs: &[ProcEntry],
    mut pid_file_id: impl FnMut(&crate::agent_binding::PaneProcess) -> Option<String>,
) -> Vec<crate::agent_binding::OtherAgent> {
    use crate::agent_sessions::AgentKind;
    let tree = ProcTree::new(procs);
    tree.root_agents()
        .into_iter()
        .filter_map(|(kind, p)| {
            let kind = AgentKind::from_kind(kind)?;
            let as_pane = crate::agent_binding::PaneProcess {
                pid: p.pid,
                start_secs: p.start_secs,
                argv: p.argv.clone(),
                subtree: BTreeSet::from([p.pid]),
            };
            let known_id = crate::agent_binding::argv_session_id(kind, &p.argv).or_else(|| {
                if kind == AgentKind::Claude {
                    pid_file_id(&as_pane)
                } else {
                    None
                }
            });
            Some(crate::agent_binding::OtherAgent {
                kind,
                pid: p.pid,
                start_secs: p.start_secs,
                cwd: p.cwd.clone(),
                known_id,
            })
        })
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

    use crate::agent_scan_disk::claude_pid_file_id;
    use crate::agent_sessions::SharedSessions;
    use crate::agents::SharedAgents;
    use crate::commands::{emit_agents_changed, flush_sessions, observe_pane_session, AppState};

    std::thread::Builder::new()
        .name("ymux-agent-scan".into())
        .spawn(move || {
            let mut sys = System::new();
            // `cwd` is read once per process, like exe and argv: it is how an
            // agent in some other terminal is told apart from one that could
            // have written a transcript in a pane's directory
            // (`agent_binding::guess_transcript`).
            let refresh = ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_cwd(UpdateKind::OnlyIfNotSet);
            loop {
                std::thread::sleep(SCAN_INTERVAL);
                let shells = app.state::<AppState>().pty.pids_snapshot();
                let (found, labels, pane_agents, others) = if shells.is_empty() {
                    (HashMap::new(), HashMap::new(), HashMap::new(), Vec::new())
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
                            start_secs: p.start_time(),
                            cwd: p.cwd().map(|c| c.to_string_lossy().into_owned()),
                        })
                        .collect();
                    let pane_agents = scan_pane_agents(&shells, &procs);
                    // Only worth enumerating every agent on the machine when a
                    // pane has one whose conversation may need guessing.
                    let others = if pane_agents.is_empty() {
                        Vec::new()
                    } else {
                        other_agents(&procs, claude_pid_file_id)
                    };
                    (
                        scan_panes(&shells, &procs),
                        scan_labels(&shells, &procs),
                        pane_agents,
                        others,
                    )
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
                // Ask before `apply_scan`, which drops the state of a pane
                // whose agent has gone.
                let exited = reg.exited_panes(&live, &found);
                if reg.apply_scan(&live, &found) {
                    // Under the lock, so this can't race a hook-driven emit
                    // out of order (see `commands::apply_agent_hook`).
                    emit_agents_changed(&app, &reg.snapshot());
                }
                let observations: Vec<(Uuid, String, Option<String>)> = found
                    .iter()
                    .filter(|(id, _)| live.contains(id))
                    .map(|(id, kind)| {
                        (
                            *id,
                            kind.clone(),
                            reg.hook_session_id(*id).map(str::to_string),
                        )
                    })
                    .collect();
                // Claude's own pid file for each pane's agent, read outside
                // every lock (it is a file open per Claude pane per tick).
                let pid_file_ids: HashMap<Uuid, String> = pane_agents
                    .iter()
                    .filter(|(_, a)| a.kind == "claude")
                    .filter_map(|(id, a)| claude_pid_file_id(&a.process).map(|s| (*id, s)))
                    .collect();
                // Registry lock first, session lock second — the same order
                // `apply_agent_hook` uses, so the two threads cannot deadlock.
                drop(reg);
                let sessions = app.state::<SharedSessions>();
                let mut tracker = sessions.0.lock();
                for pane in exited {
                    tracker.note_agent_exit(pane);
                }
                for (id, kind, hook_id) in observations {
                    // The scan cannot see what the agent is doing, only that
                    // it is there, so it reports no status: a hook's stays,
                    // and a record with none starts at `working` (the
                    // conservative answer for `interrupted`).
                    observe_pane_session(
                        &app,
                        &mut tracker,
                        crate::commands::PaneSessionInput {
                            pane_id: id,
                            kind: &kind,
                            status: None,
                            hook_session_id: hook_id,
                            process: pane_agents.get(&id).map(|a| a.process.clone()),
                            pid_file_session_id: pid_file_ids.get(&id).cloned(),
                            others: &others,
                        },
                    );
                }
                // A resume that never produced a running agent is declined;
                // one the scan (or a hook) just confirmed no longer needs
                // the pane's old scrollback.
                tracker.expire_pending(crate::agent_sessions::now_secs());
                let confirmed = tracker.take_confirmed();
                flush_sessions(&mut tracker);
                drop(tracker);
                for pane in confirmed {
                    if let Err(e) = crate::scrollback::delete_blob(&pane.to_string()) {
                        tracing::warn!(error = %e, %pane, "deleting a resumed pane's scrollback failed");
                    }
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
            start_secs: 0,
            cwd: None,
        }
    }

    #[test]
    fn pane_agent_carries_its_process_identity_and_subtree() {
        // shell(10) -> node claude-code(12) -> rg(13)
        let mut procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(
                12,
                Some(10),
                "node",
                &[
                    "node",
                    "/n/@anthropic-ai/claude-code/cli.js",
                    "--resume",
                    "abc-1",
                ],
            ),
            proc(13, Some(12), "rg", &["rg"]),
        ];
        procs[1].start_secs = 1_000;
        let id = Uuid::new_v4();
        let shells: HashMap<Uuid, u32> = [(id, 10)].into_iter().collect();
        let found = scan_pane_agents(&shells, &procs);
        let a = &found[&id];
        assert_eq!(a.kind, "claude");
        assert_eq!(a.process.pid, 12);
        assert_eq!(a.process.start_secs, 1_000);
        assert_eq!(a.process.argv[2], "--resume");
        assert_eq!(
            a.process.subtree.iter().copied().collect::<Vec<_>>(),
            vec![12, 13]
        );
    }

    #[test]
    fn agents_in_panes_of_a_ymux_started_from_an_agent_are_still_agents() {
        // outer claude(1) -> node(2) -> ymux(3) -> pwsh(4) -> claude(5)
        //                                       -> pwsh(6) -> claude(7)
        let procs = vec![
            proc(1, None, "claude", &["claude"]),
            proc(2, Some(1), "node", &["node", "vite.js"]),
            proc(3, Some(2), "ymux", &["ymux"]),
            proc(4, Some(3), "pwsh", &[]),
            proc(5, Some(4), "claude", &["claude"]),
            proc(6, Some(3), "pwsh", &[]),
            proc(7, Some(6), "claude", &["claude"]),
        ];
        let pids: Vec<u32> = other_agents(&procs, |_| None)
            .iter()
            .map(|o| o.pid)
            .collect();
        assert_eq!(pids, vec![1, 5, 7]);
    }

    #[test]
    fn a_claude_daemon_chain_is_one_agent() {
        // Seen in the wild: `claude daemon run` -> `claude --bg-pty-host … --
        // claude --session-id …` -> `claude --session-id …`.
        let procs = vec![
            proc(10, Some(1), "claude", &["claude", "daemon", "run"]),
            proc(11, Some(10), "claude", &["claude", "--bg-pty-host"]),
            proc(12, Some(11), "claude", &["claude", "--session-id", "abc-1"]),
        ];
        let pids: Vec<u32> = other_agents(&procs, |_| None)
            .iter()
            .map(|o| o.pid)
            .collect();
        assert_eq!(pids, vec![10]);
    }

    /// The scan's refresh kind must actually populate what binding relies on:
    /// with `start_time` left at 0, "began at or after the process started"
    /// would accept yesterday's transcript, and every pid file would fail its
    /// start-time check.
    #[test]
    fn the_scan_refresh_populates_start_time_and_cwd() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let refresh = ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet);
        let me = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[me]), true, refresh);
        let p = sys.process(me).expect("this test process");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(p.start_time() > 0, "start_time must be populated");
        assert!(p.start_time() <= now + 1);
        assert!(p.cwd().is_some(), "cwd must be populated");
    }

    #[test]
    fn other_agents_lists_each_running_agent_once_with_what_it_is_known_to_hold() {
        let id = "aaaaaaaa-0000-0000-0000-00000000000a";
        let mut procs = vec![
            // A node-hosted Codex whose native child also matches: one agent.
            proc(
                20,
                Some(1),
                "node",
                &["node", "/n/@openai/codex/bin/codex.js"],
            ),
            proc(21, Some(20), "codex", &["codex"]),
            // A Claude in some other terminal, resumed by id.
            proc(30, Some(2), "claude", &["claude", "--resume", id]),
            // A Claude with nothing in its argv; its pid file says.
            proc(40, Some(3), "claude", &["claude"]),
            // Gemini: no resume story, so not a competitor for anything.
            proc(50, Some(4), "gemini", &["gemini"]),
        ];
        procs[3].cwd = Some("/w".into());
        let others = other_agents(&procs, |p| (p.pid == 40).then(|| "bbbb-1".to_string()));
        let pids: Vec<u32> = others.iter().map(|o| o.pid).collect();
        assert_eq!(pids, vec![20, 30, 40]);
        assert_eq!(others[0].known_id, None);
        assert_eq!(others[1].known_id.as_deref(), Some(id));
        assert_eq!(others[2].known_id.as_deref(), Some("bbbb-1"));
        assert_eq!(others[2].cwd.as_deref(), Some("/w"));
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
        // macOS/Linux npm installs run through the bin symlink's shebang,
        // so the script argument is the symlink, not the package path.
        let brew = argv(&["node", "/opt/homebrew/bin/claude"]);
        assert_eq!(match_agent("node", &brew), Some("claude"));
        let gemini = argv(&["node", "--no-warnings", "/usr/local/bin/gemini"]);
        assert_eq!(match_agent("node", &gemini), Some("gemini"));
        let codex = argv(&["node", "/Users/me/.npm-global/bin/codex", "resume"]);
        assert_eq!(match_agent("node", &codex), Some("codex"));
        // Host flags before the script are skipped.
        let flagged = argv(&[
            "node",
            "--max-old-space-size=4096",
            "/home/x/.npm-global/lib/node_modules/@openai/codex/bin/codex.js",
        ]);
        assert_eq!(match_agent("node", &flagged), Some("codex"));
    }

    #[test]
    fn matcher_uses_argv0_when_the_executable_path_does_not_name_the_agent() {
        // Native Claude: version-named executable, `process.title = "claude"`
        // overwrites argv, so only argv[0] survives.
        assert_eq!(match_agent("2.1", &argv(&["claude"])), Some("claude"));
        assert_eq!(
            match_agent("2.1", &argv(&["/Users/me/.local/bin/claude", "--resume"])),
            Some("claude")
        );
        // `claude` as argv[0] is trusted on any stem.
        assert_eq!(match_agent("x", &argv(&["claude"])), Some("claude"));
        assert_eq!(
            match_agent("x", &argv(&[r"C:\tools\Claude.EXE"])),
            Some("claude")
        );
        // Other agent names only on a version-number stem.
        assert_eq!(match_agent("0.42.1", &argv(&["codex"])), Some("codex"));
    }

    #[test]
    fn matcher_ignores_other_agent_names_in_argv0_of_unrelated_executables() {
        for (stem, a0) in [
            ("python", "aider"),
            ("git", "amp"),
            ("x", r"C:\tools\Codex.EXE"),
            ("cmd", "gemini.cmd"),
            ("v2", "codex"),
            ("", "opencode"),
            ("...", "amp"),
        ] {
            assert_eq!(match_agent(stem, &argv(&[a0])), None, "{stem} {a0}");
        }
        // Their own exe stem still matches normally.
        assert_eq!(match_agent("aider", &argv(&["aider"])), Some("aider"));
        assert_eq!(match_agent("amp", &argv(&["/usr/bin/amp"])), Some("amp"));
    }

    #[test]
    fn matcher_basename_rules_reject_lookalikes() {
        // The script's basename must equal an agent name exactly.
        for script in [
            "/x/claude-helper.js",
            "/x/claude.js",
            "/x/bin/claudette",
            "/x/claude/server.js",
        ] {
            assert_eq!(
                match_agent("node", &argv(&["node", script])),
                None,
                "{script}"
            );
        }
        // argv[0] lookalikes, and an agent name that is only a later arg.
        assert_eq!(match_agent("zsh", &argv(&["-zsh"])), None);
        assert_eq!(match_agent("x", &argv(&["claude-helper"])), None);
        assert_eq!(
            match_agent(
                "node",
                &argv(&["node", "server.js", "/usr/local/bin/claude"])
            ),
            None
        );
        // A basename match only applies to a script host's script.
        assert_eq!(
            match_agent("python", &argv(&["python", "/usr/local/bin/claude"])),
            None
        );
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
            proc(11, Some(10), "vim", &["vim", "/w/README.md"]),
            proc(20, Some(1), "pwsh", &[]),
            proc(30, Some(1), "zsh", &[]),
            proc(31, Some(30), "claude", &["claude"]),
        ];
        let shells: HashMap<Uuid, u32> = [(a, 10), (b, 20), (c, 30)].into_iter().collect();
        let found = scan_labels(&shells, &procs);
        assert_eq!(found.get(&a).map(String::as_str), Some("vim"));
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
    fn label_falls_back_to_the_deepest_descendant_without_a_known_program() {
        // Nothing here is a known agent, so the
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
