//! The top bar's "+" launcher: which agent CLIs are installed, and the flag
//! that starts each one with its permission prompts bypassed.
//!
//! The table below is the single source of truth for the launcher. The
//! frontend only turns a [`DetectedAgent`] into typed text (quoting the path
//! for the pane's shell, `src/workspace/agentLaunch.ts`); nothing here starts
//! an agent, so the `detect_agents` command that wraps [`detect_agents`]
//! cannot run one even if a web page reached it. (The only process it spawns
//! is the fixed login-shell `PATH` probe, whose command line takes no input.)
//!
//! Detection is a `which` over three tiers of directories:
//!
//! 1. the user's **login-shell `PATH`** (Unix only), probed once per run by
//!    [`login_shell_path`] — a Finder-launched macOS app gets
//!    `/usr/bin:/bin:/usr/sbin:/sbin`, so an `npm i -g` under nvm/fnm/volta,
//!    `~/.npm-global` or pnpm is invisible to it even though every terminal
//!    finds it;
//! 2. this process's own `PATH`;
//! 3. a few known install locations ([`known_dirs`], [`version_manager_dirs`])
//!    for when the probe fails or times out.
//!
//! An agent found in tier 1 or 2 is typed by **bare name**: panes run login
//! shells, whose `PATH` is what tier 1 measured, and a bare name is also what
//! the process scan and resume match on. One found only in tier 3 is typed by
//! its absolute path, since nothing says the pane's shell has that directory.
//! (Caveat: the probe runs `$SHELL`; a pane running a different shell may
//! lack a tier-1 directory. The PTY environment is never touched.)
//!
//! Pure apart from [`detect_agents`], [`login_shell_path`] and
//! [`is_launchable`], which read the environment, spawn the probe shell and
//! read the filesystem.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// One launchable agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDef {
    /// Stable id, also the `agent_scan` kind where the scan knows it.
    pub id: &'static str,
    /// Shown in the menu.
    pub name: &'static str,
    /// Executable names to look for, in order of preference.
    pub exes: &'static [&'static str],
    /// Appended to the command. Empty = the agent has no bypass flag for its
    /// interactive mode and starts with its normal prompts.
    pub bypass_args: &'static [&'static str],
    /// An extra caveat for the tooltip, as an i18n key.
    pub note: Option<&'static str>,
}

/// Every agent the launcher offers. `cursor-agent` and `auggie` are
/// deliberately absent: their bypass flags are unverified.
pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        name: "Claude Code",
        exes: &["claude"],
        bypass_args: &["--dangerously-skip-permissions"],
        note: None,
    },
    AgentDef {
        id: "codex",
        name: "Codex",
        exes: &["codex"],
        bypass_args: &["--dangerously-bypass-approvals-and-sandbox"],
        note: Some("launcher.noteCodexSandbox"),
    },
    AgentDef {
        id: "gemini",
        name: "Gemini CLI",
        exes: &["gemini"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "kimi",
        name: "Kimi CLI",
        exes: &["kimi"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "qwen",
        name: "Qwen Code",
        exes: &["qwen"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "aider",
        name: "Aider",
        exes: &["aider"],
        bypass_args: &["--yes-always"],
        note: None,
    },
    AgentDef {
        id: "amp",
        name: "Amp",
        exes: &["amp"],
        bypass_args: &["--dangerously-allow-all"],
        note: None,
    },
    AgentDef {
        id: "copilot",
        name: "GitHub Copilot CLI",
        exes: &["copilot"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "crush",
        name: "Crush",
        exes: &["crush"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "cline",
        name: "Cline CLI",
        exes: &["cline"],
        bypass_args: &["--yolo"],
        note: None,
    },
    AgentDef {
        id: "droid",
        name: "Factory Droid",
        exes: &["droid"],
        bypass_args: &["--skip-permissions-unsafe"],
        note: None,
    },
    AgentDef {
        id: "kiro-cli",
        name: "Kiro CLI",
        exes: &["kiro-cli"],
        bypass_args: &["--trust-all-tools"],
        note: None,
    },
    AgentDef {
        id: "opencode",
        name: "opencode",
        exes: &["opencode"],
        bypass_args: &[],
        note: None,
    },
];

/// An installed agent, as the frontend receives it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DetectedAgent {
    pub id: String,
    pub name: String,
    /// The bare executable name, typed as-is when it is on `PATH`.
    pub program: String,
    /// The absolute path, set only when the agent is *not* on `PATH` and has
    /// to be launched by path.
    pub path: Option<String>,
    pub bypass_args: Vec<String>,
    pub note: Option<String>,
    /// The only file found is a `.ps1` shim: it runs from PowerShell, but cmd
    /// does not resolve it (`.ps1` is not in `PATHEXT`) and a typed `.ps1`
    /// path opens via file association and silently does nothing. The
    /// launcher offers such an agent only in a PowerShell pane.
    pub powershell_only: bool,
}

/// Windows launchable extensions, most preferred first. A `.ps1` npm shim
/// runs only from PowerShell, so it loses to a `.cmd` beside it.
const WINDOWS_EXTS: &[&str] = &[".exe", ".cmd", ".bat", ".ps1"];

/// The file names `exe` may have on disk.
pub fn candidate_names(exe: &str, windows: bool) -> Vec<String> {
    if windows {
        WINDOWS_EXTS.iter().map(|e| format!("{exe}{e}")).collect()
    } else {
        vec![exe.to_string()]
    }
}

/// `PATH` split into directories. Entries wrapped in `"…"` (legal and not
/// rare on Windows) are unwrapped; empty ones dropped.
pub fn path_dirs(path_var: Option<&OsStr>) -> Vec<PathBuf> {
    let Some(v) = path_var else {
        return Vec::new();
    };
    std::env::split_paths(v)
        .filter_map(|p| {
            let s = p.to_string_lossy();
            let s = s.trim();
            let s = s
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .unwrap_or(s);
            (!s.is_empty()).then(|| PathBuf::from(s))
        })
        .collect()
}

/// Install locations outside a GUI app's `PATH` that agent installers use.
/// `appdata` / `localappdata` are Windows' `%APPDATA%` / `%LOCALAPPDATA%`.
pub fn known_dirs(
    home: Option<&Path>,
    appdata: Option<&Path>,
    localappdata: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = home {
        dirs.push(h.join(".local").join("bin"));
        dirs.push(h.join(".claude").join("local"));
        dirs.push(h.join(".bun").join("bin"));
        dirs.push(h.join(".kimi-code").join("bin"));
        dirs.push(h.join(".volta").join("bin"));
        dirs.push(h.join(".cargo").join("bin"));
        if cfg!(windows) {
            dirs.push(h.join("scoop").join("shims"));
        } else {
            // `npm config set prefix ~/.npm-global`, the npm docs' advice.
            dirs.push(h.join(".npm-global").join("bin"));
            // pnpm's global bin: macOS, then Linux.
            dirs.push(h.join("Library").join("pnpm"));
            dirs.push(h.join(".local").join("share").join("pnpm"));
        }
    }
    if let Some(a) = appdata {
        dirs.push(a.join("npm"));
    }
    if let Some(l) = localappdata {
        dirs.push(l.join("pnpm"));
    }
    if !cfg!(windows) {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

/// `v20.11.1` / `20.11.1` as numbers, for newest-first ordering. `None` for
/// anything else (`system`, an alias directory, junk).
fn version_key(name: &str) -> Option<Vec<u64>> {
    let v = name.strip_prefix('v').unwrap_or(name);
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// `<root>/<version>/<sub…>` for every entry name in `versions`, newest
/// version first (numerically, so `v10` beats `v9`); unparsable names go
/// last, in name order. That is nvm's `versions/node/*/bin` and fnm's
/// `node-versions/*/installation/bin`.
pub fn versioned_bins(root: &Path, versions: &[String], sub: &[&str]) -> Vec<PathBuf> {
    let mut names: Vec<&String> = versions.iter().collect();
    names.sort_by(|a, b| match (version_key(a), version_key(b)) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    });
    names
        .into_iter()
        .map(|n| sub.iter().fold(root.join(n), |p, s| p.join(s)))
        .collect()
}

/// Node version managers' per-version bin directories (Unix only): nvm
/// under `$NVM_DIR` (default `~/.nvm`), fnm under its macOS, XDG and legacy
/// data dirs. `list` returns a directory's entry names (empty if unreadable).
pub fn version_manager_dirs(
    home: Option<&Path>,
    nvm_dir: Option<&Path>,
    windows: bool,
    list: &dyn Fn(&Path) -> Vec<String>,
) -> Vec<PathBuf> {
    if windows {
        return Vec::new();
    }
    let Some(h) = home else {
        return Vec::new();
    };
    let nvm = nvm_dir.map_or_else(|| h.join(".nvm"), Path::to_path_buf);
    let node = nvm.join("versions").join("node");
    let mut out = versioned_bins(&node, &list(&node), &["bin"]);
    for fnm in [
        h.join("Library").join("Application Support").join("fnm"),
        h.join(".local").join("share").join("fnm"),
        h.join(".fnm"),
    ] {
        let root = fnm.join("node-versions");
        out.extend(versioned_bins(
            &root,
            &list(&root),
            &["installation", "bin"],
        ));
    }
    out
}

/// `first` then `second`, each directory once (first occurrence wins, which
/// keeps `PATH`'s first-match semantics).
pub fn merge_dirs(first: &[PathBuf], second: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in first.iter().chain(second) {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    out
}

/// Printed around `$PATH` by the login-shell probe, so rc-file banners,
/// MOTDs and prompts around it can be cut away.
pub const PROBE_START: &str = "__YMUX_PATH_START__";
pub const PROBE_END: &str = "__YMUX_PATH_END__";

/// `s` without ANSI escape sequences: CSI (`ESC [ … final`), OSC
/// (`ESC ] … BEL` or `… ESC \`) and two-character `ESC x`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The `PATH` directories in the probe shell's stdout: the text between the
/// first [`PROBE_START`] and the next [`PROBE_END`], ANSI stripped, split on
/// `:`, keeping only absolute entries without control characters, each once.
/// Empty when either delimiter is missing — the probe failed.
pub fn parse_probe_output(out: &str) -> Vec<PathBuf> {
    let clean = strip_ansi(out);
    let Some(start) = clean.find(PROBE_START) else {
        return Vec::new();
    };
    let rest = &clean[start + PROBE_START.len()..];
    let Some(end) = rest.find(PROBE_END) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in rest[..end].split(':') {
        let entry = entry.trim();
        if !entry.starts_with('/') || entry.chars().any(char::is_control) {
            continue;
        }
        let d = PathBuf::from(entry);
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// The first launchable file for any of `names` in `dirs`: directory order
/// first (that is `PATH` semantics), then name order within a directory.
fn find_in(dirs: &[PathBuf], names: &[String], is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| is_file(p))
}

/// [`find_in`], but a `.ps1` anywhere in `dirs` only counts when nothing
/// else is found: cmd and Git Bash skip `.ps1` while resolving, so a `.cmd`
/// in a later directory is what they would run.
fn locate(dirs: &[PathBuf], names: &[String], is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let (ps1, other): (Vec<String>, Vec<String>) =
        names.iter().cloned().partition(|n| is_ps1(Path::new(n)));
    find_in(dirs, &other, is_file).or_else(|| find_in(dirs, &ps1, is_file))
}

fn is_ps1(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ps1"))
}

/// Which of [`AGENTS`] are installed, in table order. A hit on `PATH` wins
/// over one in `known`, and yields no `path` (the bare name is typed).
pub fn detect_with(
    path: &[PathBuf],
    known: &[PathBuf],
    windows: bool,
    is_file: &dyn Fn(&Path) -> bool,
) -> Vec<DetectedAgent> {
    AGENTS
        .iter()
        .filter_map(|def| {
            def.exes.iter().find_map(|exe| {
                let names = candidate_names(exe, windows);
                let (hit, on_path) = match locate(path, &names, is_file) {
                    Some(p) => (p, true),
                    None => (locate(known, &names, is_file)?, false),
                };
                let path = (!on_path).then(|| hit.to_string_lossy().into_owned());
                Some(DetectedAgent {
                    id: def.id.to_string(),
                    name: def.name.to_string(),
                    program: (*exe).to_string(),
                    path,
                    bypass_args: def.bypass_args.iter().map(|s| s.to_string()).collect(),
                    note: def.note.map(str::to_string),
                    powershell_only: is_ps1(&hit),
                })
            })
        })
        .collect()
}

/// A regular file this user could run: on Unix it needs an execute bit.
pub fn is_launchable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// How long the login-shell probe may take. A profile loading nvm, conda and
/// friends on a busy machine takes several seconds (orca measured 6–7 s).
#[cfg(unix)]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Set in the probe shell's environment, so an rc file that execs a
/// multiplexer or draws a heavy prompt can take a fast path.
#[cfg(unix)]
const PROBE_MARKER_ENV: &str = "YMUX_SHELL_PATH_PROBE";

/// Run `$SHELL -ilc` (falling back to `/bin/zsh`) with stdin closed and
/// stderr discarded, and parse the `PATH` it prints. Empty on any failure:
/// spawn error, missing delimiters, or no answer within [`PROBE_TIMEOUT`]
/// (the shell's process group is then killed). An interactive rc may print,
/// prompt or hang; with no stdin and a deadline none of that can stall us.
#[cfg(unix)]
fn probe_login_path() -> Vec<PathBuf> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let shell = std::env::var_os("SHELL")
        .filter(|s| Path::new(s).is_absolute())
        .unwrap_or_else(|| "/bin/zsh".into());
    let script =
        format!("printf '%s' '{PROBE_START}'; printf '%s' \"$PATH\"; printf '%s' '{PROBE_END}'");
    let Ok(mut child) = Command::new(&shell)
        .arg("-ilc")
        .arg(script)
        .env(PROBE_MARKER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own group, so a timeout also kills whatever the rc started.
        .process_group(0)
        .spawn()
    else {
        return Vec::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Vec::new();
    };
    let (tx, rx) = std::sync::mpsc::channel();
    // A background job the rc started can hold the pipe open past the
    // shell's exit, so the deadline is on the channel, not on the read.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(buf) => {
            let _ = child.wait();
            parse_probe_output(&String::from_utf8_lossy(&buf))
        }
        Err(_) => {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
            Vec::new()
        }
    }
}

/// The user's login-shell `PATH`, probed once per run and cached (a failed
/// probe caches as empty); `refresh` probes again. Always empty on Windows,
/// where a GUI app inherits the user's real `PATH`. Blocks for up to the
/// probe timeout on a cold cache — call it off the main thread.
pub fn login_shell_path(refresh: bool) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        use std::sync::Mutex;
        static CACHE: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);
        // Held across the probe: a concurrent caller waits for this answer
        // instead of starting a second shell.
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if refresh || cache.is_none() {
            *cache = Some(probe_login_path());
        }
        cache.clone().unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        let _ = refresh;
        Vec::new()
    }
}

/// Entry names of `dir`, empty when it can't be read.
fn list_dir(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// [`detect_with`] over the login-shell `PATH` then this process's `PATH`
/// (both typed by bare name), and the known install directories.
/// `refresh` re-runs the login-shell probe (the launcher's "Rescan").
/// Spawns a shell and touches the filesystem — call it off the main thread.
pub fn detect_agents(refresh: bool) -> Vec<DetectedAgent> {
    let process = path_dirs(std::env::var_os("PATH").as_deref());
    let path = merge_dirs(&login_shell_path(refresh), &process);
    let (appdata, localappdata) = if cfg!(windows) {
        (
            std::env::var_os("APPDATA").map(PathBuf::from),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        )
    } else {
        (None, None)
    };
    let home = dirs::home_dir();
    let nvm_dir = std::env::var_os("NVM_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let mut known = known_dirs(home.as_deref(), appdata.as_deref(), localappdata.as_deref());
    known.extend(version_manager_dirs(
        home.as_deref(),
        nvm_dir.as_deref(),
        cfg!(windows),
        &list_dir,
    ));
    detect_with(&path, &known, cfg!(windows), &is_launchable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn fs(files: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    fn def(id: &str) -> &'static AgentDef {
        AGENTS.iter().find(|a| a.id == id).unwrap()
    }

    #[test]
    fn table_has_the_verified_bypass_flags() {
        for (id, args) in [
            ("claude", &["--dangerously-skip-permissions"][..]),
            ("codex", &["--dangerously-bypass-approvals-and-sandbox"]),
            ("gemini", &["--yolo"]),
            ("kimi", &["--yolo"]),
            ("qwen", &["--yolo"]),
            ("aider", &["--yes-always"]),
            ("amp", &["--dangerously-allow-all"]),
            ("copilot", &["--yolo"]),
            ("crush", &["--yolo"]),
            ("cline", &["--yolo"]),
            ("droid", &["--skip-permissions-unsafe"]),
            ("kiro-cli", &["--trust-all-tools"]),
            ("opencode", &[]),
        ] {
            assert_eq!(def(id).bypass_args, args, "{id}");
        }
        assert_eq!(AGENTS.len(), 13);
        assert_eq!(def("codex").note, Some("launcher.noteCodexSandbox"));
    }

    #[test]
    fn unverified_agents_are_not_offered() {
        for id in ["cursor-agent", "auggie"] {
            assert!(AGENTS.iter().all(|a| a.id != id && !a.exes.contains(&id)));
        }
    }

    #[test]
    fn ids_are_unique() {
        let ids: HashSet<_> = AGENTS.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), AGENTS.len());
    }

    #[test]
    fn candidate_names_per_platform() {
        assert_eq!(candidate_names("codex", false), vec!["codex"]);
        assert_eq!(
            candidate_names("codex", true),
            vec!["codex.exe", "codex.cmd", "codex.bat", "codex.ps1"]
        );
    }

    #[test]
    fn path_dirs_unwraps_quotes_and_drops_empties() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let raw = format!("/a{sep}{sep}\"/b c\"{sep} ");
        let dirs = path_dirs(Some(OsStr::new(&raw)));
        assert_eq!(dirs, vec![PathBuf::from("/a"), PathBuf::from("/b c")]);
        assert!(path_dirs(None).is_empty());
    }

    #[test]
    fn found_on_path_types_the_bare_name() {
        let path = vec![PathBuf::from("/usr/bin")];
        let got = detect_with(&path, &[], false, &fs(&["/usr/bin/claude"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "claude");
        assert_eq!(got[0].program, "claude");
        assert_eq!(got[0].path, None);
        assert_eq!(got[0].bypass_args, vec!["--dangerously-skip-permissions"]);
    }

    #[test]
    fn found_only_in_a_known_dir_carries_the_absolute_path() {
        let path = vec![PathBuf::from("/usr/bin")];
        let known = vec![PathBuf::from("/opt/homebrew/bin")];
        let got = detect_with(&path, &known, false, &fs(&["/opt/homebrew/bin/gemini"]));
        assert_eq!(got.len(), 1);
        let want = PathBuf::from("/opt/homebrew/bin").join("gemini");
        assert_eq!(
            got[0].path.as_deref(),
            Some(want.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn path_hit_beats_known_dir_hit() {
        let path = vec![PathBuf::from("/p")];
        let known = vec![PathBuf::from("/k")];
        let got = detect_with(&path, &known, false, &fs(&["/p/codex", "/k/codex"]));
        assert_eq!(got[0].path, None);
    }

    #[test]
    fn windows_prefers_exe_then_cmd_within_a_dir_and_accepts_a_lone_ps1() {
        let npm = PathBuf::from("npm");
        let known = vec![npm.clone()];
        let both = |a: &str, b: &str| {
            let (a, b) = (npm.join(a), npm.join(b));
            move |p: &Path| p == a || p == b
        };
        let got = detect_with(&[], &known, true, &both("codex.ps1", "codex.cmd"));
        let cmd = npm.join("codex.cmd");
        assert_eq!(got[0].path.as_deref(), Some(cmd.to_string_lossy().as_ref()));

        let ps1 = npm.join("gemini.ps1");
        let only = {
            let ps1 = ps1.clone();
            move |p: &Path| p == ps1
        };
        let got = detect_with(&[], &known, true, &only);
        assert_eq!(got[0].path.as_deref(), Some(ps1.to_string_lossy().as_ref()));
    }

    #[test]
    fn a_lone_ps1_is_powershell_only_and_loses_to_a_later_cmd() {
        let path = vec![PathBuf::from("a"), PathBuf::from("b")];
        let ps1 = PathBuf::from("a").join("codex.ps1");
        let cmd = PathBuf::from("b").join("codex.cmd");
        let only_ps1 = {
            let ps1 = ps1.clone();
            move |p: &Path| p == ps1
        };
        let got = detect_with(&path, &[], true, &only_ps1);
        assert_eq!(got[0].path, None);
        assert!(got[0].powershell_only);

        let both = move |p: &Path| p == ps1 || p == cmd;
        let got = detect_with(&path, &[], true, &both);
        assert!(!got[0].powershell_only);

        let bare = PathBuf::from("a").join("codex");
        let unix = detect_with(&path, &[], false, &move |p: &Path| p == bare);
        assert!(!unix.is_empty() && !unix[0].powershell_only);
    }

    #[test]
    fn path_order_wins_over_extension_order() {
        // An earlier PATH directory is what the shell would run, whatever
        // extension it has.
        let path = vec![PathBuf::from("a"), PathBuf::from("b")];
        let (a, b) = (
            PathBuf::from("a").join("claude.cmd"),
            PathBuf::from("b").join("claude.exe"),
        );
        let hits = move |p: &Path| p == a || p == b;
        assert_eq!(
            find_in(&path, &candidate_names("claude", true), &hits),
            Some(PathBuf::from("a").join("claude.cmd"))
        );
    }

    #[test]
    fn results_follow_table_order_and_skip_missing() {
        let path = vec![PathBuf::from("/b")];
        let got = detect_with(
            &path,
            &[],
            false,
            &fs(&["/b/opencode", "/b/claude", "/b/kimi"]),
        );
        let ids: Vec<_> = got.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["claude", "kimi", "opencode"]);
        assert!(got[2].bypass_args.is_empty());
    }

    #[test]
    fn known_dirs_cover_the_installer_locations() {
        let home = PathBuf::from("/h");
        let dirs = known_dirs(Some(&home), Some(Path::new("/ad")), Some(Path::new("/lad")));
        for d in [
            home.join(".local").join("bin"),
            home.join(".claude").join("local"),
            home.join(".bun").join("bin"),
            home.join(".kimi-code").join("bin"),
            home.join(".volta").join("bin"),
            home.join(".cargo").join("bin"),
            PathBuf::from("/ad").join("npm"),
            PathBuf::from("/lad").join("pnpm"),
        ] {
            assert!(dirs.contains(&d), "{d:?}");
        }
        if cfg!(windows) {
            assert!(dirs.contains(&home.join("scoop").join("shims")));
        } else {
            for d in [
                home.join(".npm-global").join("bin"),
                home.join("Library").join("pnpm"),
                home.join(".local").join("share").join("pnpm"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ] {
                assert!(dirs.contains(&d), "{d:?}");
            }
        }
        assert!(known_dirs(None, None, None).len() <= 2);
    }

    #[test]
    fn versioned_bins_are_newest_first_numerically() {
        let root = PathBuf::from("/n");
        let names: Vec<String> = ["v9.11.2", "system", "v20.11.1", "v20.9.0", "v18.19.0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = versioned_bins(&root, &names, &["bin"]);
        let want: Vec<PathBuf> = ["v20.11.1", "v20.9.0", "v18.19.0", "v9.11.2", "system"]
            .iter()
            .map(|v| root.join(v).join("bin"))
            .collect();
        assert_eq!(got, want);
        let fnm = versioned_bins(&root, &["v22.1.0".into()], &["installation", "bin"]);
        assert_eq!(
            fnm,
            vec![root.join("v22.1.0").join("installation").join("bin")]
        );
    }

    #[test]
    fn version_manager_dirs_cover_nvm_and_fnm_on_unix_only() {
        let home = PathBuf::from("/h");
        let nvm_node = home.join(".nvm").join("versions").join("node");
        let fnm_mac = home
            .join("Library")
            .join("Application Support")
            .join("fnm")
            .join("node-versions");
        let list = {
            let (nvm_node, fnm_mac) = (nvm_node.clone(), fnm_mac.clone());
            move |p: &Path| -> Vec<String> {
                if p == nvm_node {
                    vec!["v18.0.0".into(), "v22.3.0".into()]
                } else if p == fnm_mac {
                    vec!["v20.0.0".into()]
                } else {
                    Vec::new()
                }
            }
        };
        let got = version_manager_dirs(Some(&home), None, false, &list);
        assert_eq!(
            got,
            vec![
                nvm_node.join("v22.3.0").join("bin"),
                nvm_node.join("v18.0.0").join("bin"),
                fnm_mac.join("v20.0.0").join("installation").join("bin"),
            ]
        );
        // $NVM_DIR relocates nvm.
        let custom = PathBuf::from("/opt/nvm");
        let seen = std::cell::RefCell::new(Vec::new());
        let spy = |p: &Path| -> Vec<String> {
            seen.borrow_mut().push(p.to_path_buf());
            Vec::new()
        };
        version_manager_dirs(Some(&home), Some(&custom), false, &spy);
        assert!(seen
            .borrow()
            .contains(&custom.join("versions").join("node")));
        assert!(version_manager_dirs(Some(&home), None, true, &list).is_empty());
        assert!(version_manager_dirs(None, None, false, &list).is_empty());
    }

    #[test]
    fn probe_output_survives_banners_and_ansi() {
        let out = format!(
            "\x1b[1;32mWelcome to fish-ish zsh!\x1b[0m\nLast login: today\n\
             \x1b]0;title\x07{PROBE_START}/Users/u/.nvm/versions/node/v22.3.0/bin:\
             /opt/homebrew/bin:/usr/bin:/opt/homebrew/bin{PROBE_END}\x1b[?2004l\nbye\n"
        );
        assert_eq!(
            parse_probe_output(&out),
            vec![
                PathBuf::from("/Users/u/.nvm/versions/node/v22.3.0/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
            ]
        );
    }

    #[test]
    fn probe_output_drops_relative_empty_and_control_entries() {
        let out = format!("{PROBE_START}.:bin::/ok: /spaced :/bad\x07x:~/.local/bin{PROBE_END}");
        assert_eq!(
            parse_probe_output(&out),
            vec![PathBuf::from("/ok"), PathBuf::from("/spaced")]
        );
    }

    #[test]
    fn probe_output_without_both_delimiters_is_empty() {
        assert!(parse_probe_output("").is_empty());
        assert!(parse_probe_output("/usr/bin:/bin").is_empty());
        assert!(parse_probe_output(&format!("{PROBE_START}/usr/bin")).is_empty());
        assert!(parse_probe_output(&format!("/usr/bin{PROBE_END}")).is_empty());
        assert!(parse_probe_output(&format!("{PROBE_START}{PROBE_END}")).is_empty());
    }

    #[test]
    fn login_path_comes_first_and_duplicates_collapse() {
        let login = vec![PathBuf::from("/nvm/bin"), PathBuf::from("/usr/bin")];
        let process = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        assert_eq!(
            merge_dirs(&login, &process),
            vec![
                PathBuf::from("/nvm/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin")
            ]
        );
    }

    #[test]
    fn found_on_the_login_path_types_the_bare_name() {
        // The Finder-launched case: codex under nvm, on the login-shell PATH
        // only; the known-dir copy must not win and force an absolute path.
        let login = vec![PathBuf::from("/nvm/bin")];
        let process = vec![PathBuf::from("/usr/bin")];
        let known = vec![PathBuf::from("/k")];
        let path = merge_dirs(&login, &process);
        let got = detect_with(&path, &known, false, &fs(&["/nvm/bin/codex", "/k/codex"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "codex");
        assert_eq!(got[0].path, None);
        // Without the probe it would only be found by absolute path.
        let got = detect_with(&process, &known, false, &fs(&["/k/codex"]));
        assert_eq!(
            got[0].path.as_deref(),
            Some(PathBuf::from("/k").join("codex").to_string_lossy().as_ref())
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn no_login_shell_probe_off_unix() {
        assert!(login_shell_path(false).is_empty());
        assert!(login_shell_path(true).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_probe_never_panics_and_is_absolute() {
        // Whatever this machine's $SHELL does, the answer is empty or all
        // absolute; the cache hands back the same thing.
        let first = login_shell_path(false);
        assert!(first.iter().all(|p| p.is_absolute()));
        assert_eq!(login_shell_path(false), first);
    }

    #[test]
    fn is_launchable_rejects_directories_and_missing_files() {
        let dir = std::env::temp_dir();
        assert!(!is_launchable(&dir));
        assert!(!is_launchable(&dir.join("ymux-definitely-missing-agent")));
    }
}
