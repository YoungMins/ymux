//! The top bar's "+" launcher: which agent CLIs are installed, and the flag
//! that starts each one with its permission prompts bypassed.
//!
//! The table below is the single source of truth for the launcher. The
//! frontend only turns a [`DetectedAgent`] into typed text (quoting the path
//! for the pane's shell, `src/workspace/agentLaunch.ts`); nothing here spawns
//! anything, so the `detect_agents` command that wraps [`detect_agents`]
//! cannot start a process even if a web page reached it.
//!
//! Detection is a `which`: every name is looked up on `PATH`, then in a few
//! install locations that a GUI app's `PATH` often lacks — a Finder-launched
//! macOS app gets `/usr/bin:/bin:/usr/sbin:/sbin`, so `/opt/homebrew/bin` or
//! `~/.local/bin` are invisible to it even though the user's login shell
//! finds them. An agent found only there is launched by absolute path.
//!
//! Pure apart from [`detect_agents`] / [`is_launchable`], which read the
//! environment and the filesystem.

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
pub fn known_dirs(home: Option<&Path>, appdata: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = home {
        dirs.push(h.join(".local").join("bin"));
        dirs.push(h.join(".claude").join("local"));
        dirs.push(h.join(".bun").join("bin"));
        dirs.push(h.join(".kimi-code").join("bin"));
    }
    if let Some(a) = appdata {
        dirs.push(a.join("npm"));
    }
    if !cfg!(windows) {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
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

/// [`detect_with`] over this process's `PATH`, home and `%APPDATA%`.
/// Touches the filesystem — call it off the main thread.
pub fn detect_agents() -> Vec<DetectedAgent> {
    let path = path_dirs(std::env::var_os("PATH").as_deref());
    let appdata = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        None
    };
    let home = dirs::home_dir();
    let known = known_dirs(home.as_deref(), appdata.as_deref());
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
        let dirs = known_dirs(Some(&home), Some(Path::new("/ad")));
        for d in [
            home.join(".local").join("bin"),
            home.join(".claude").join("local"),
            home.join(".bun").join("bin"),
            home.join(".kimi-code").join("bin"),
            PathBuf::from("/ad").join("npm"),
        ] {
            assert!(dirs.contains(&d), "{d:?}");
        }
        if !cfg!(windows) {
            assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
            assert!(dirs.contains(&PathBuf::from("/usr/local/bin")));
        }
        assert!(known_dirs(None, None).len() <= 2);
    }

    #[test]
    fn is_launchable_rejects_directories_and_missing_files() {
        let dir = std::env::temp_dir();
        assert!(!is_launchable(&dir));
        assert!(!is_launchable(&dir.join("ymux-definitely-missing-agent")));
    }
}
