//! Resolving and opening filesystem paths that the frontend lifted out of
//! terminal output.
//!
//! The frontend's path linkifier hands raw substrings of whatever a shell
//! printed — `cat`, `git log`, an agent's answer — and asks two things:
//! "does this exist?" and, on a click, "open it with the OS default
//! program". Both questions are answered here rather than in `commands.rs`
//! so the parts worth testing (validation, tilde expansion, the UNC policy,
//! the reveal-instead-of-run list) stay reachable from
//! `cargo test --no-default-features --lib -p ymux` on Linux CI.
//!
//! ## The threat model, because it is not the obvious one
//!
//! Terminal output is attacker-influenced. Anything the user `cat`s, any
//! repository they clone, any page an agent quotes can plant a string that
//! this module will be asked about. Two consequences shape the code:
//!
//!  1. **Probing is automatic.** Existence checks fire on hover, without a
//!     click. On Windows, statting `\\evil.example\x\y` makes the SMB client
//!     open a connection and offer NTLM credentials, which is a credential
//!     leak triggered by moving a mouse; an unreachable host also hangs the
//!     call for tens of seconds. So auto-probing a remote UNC host is
//!     refused unless it is a known-local pseudo-host or the host the pane's
//!     own cwd already lives on. See [`network_probe_allowed`].
//!  2. **"Open with the default program" runs executables.** `ShellExecuteW`
//!     on `evil.bat` does not open it, it executes it — and the user's
//!     mental model for clicking a link is "show me this", not "run this".
//!     Executables and scripts are therefore revealed in the file manager
//!     instead of launched. See [`should_reveal`].
//!
//! Nothing here ever builds a shell command line. `opener` uses
//! `ShellExecuteW` on Windows and `Command::new("open")` on macOS, both of
//! which take the path as an opaque argument, so a filename containing `&`,
//! `^`, `%` or a quote is inert.

use std::path::{Path, PathBuf};

/// Only the app's own main webview may ask about or open local paths.
///
/// This is not theoretical: `capabilities/browser-children.json` grants
/// `core:default` to every `eb-*` child webview on `http(s)://**`, so the
/// page loaded in an embedded browser pane can reach `invoke`. Without this
/// check any website the user visits could enumerate their filesystem
/// through the probe and open arbitrary local files through the opener.
pub fn caller_allowed(label: &str) -> bool {
    label == "main"
}

/// Longest raw candidate worth looking at. Comfortably past any real path
/// (`MAX_PATH` is 260, and even the extended limit is 32767) while keeping a
/// pathological line from turning into a long syscall.
pub const MAX_RAW_LEN: usize = 4096;

/// Most candidates probed in one request. The frontend sends at most one
/// row's worth (24), so this is a backstop against a caller that does not.
pub const MAX_BATCH: usize = 32;

/// How long a batch of existence checks may take before it is abandoned.
/// A mapped network drive that has gone away blocks `metadata` for tens of
/// seconds; a hover must not be able to wedge anything for that long.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// A resolved, existing path, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPath {
    /// Absolute, with Windows' verbatim `\\?\` prefix stripped so it is both
    /// readable in the tooltip and acceptable to `ShellExecuteW`.
    pub absolute: String,
    /// Directories open in the OS file manager rather than an application.
    pub is_dir: bool,
}

/// Why a raw candidate is not worth touching at all, or `None` if it is.
///
/// Existence is the main filter, but deliberately not the only one: a
/// control character must not reach the opener even if some filesystem
/// somewhere would accept it in a name.
pub fn reject_reason(raw: &str) -> Option<&'static str> {
    if raw.is_empty() {
        return Some("empty path");
    }
    if raw.len() > MAX_RAW_LEN {
        return Some("path too long");
    }
    if raw.chars().any(|c| c.is_control()) {
        return Some("path contains a control character");
    }
    None
}

/// How a path's *syntax* engages Windows' UNC and device namespaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UncKind {
    /// An ordinary path. Nothing special about it.
    Local,
    /// `\\?\C:\…` — the verbatim spelling of a local drive path.
    VerbatimLocal,
    /// `\\.\…`, or a `\\?\…` form that is neither a drive nor a UNC share.
    /// The Win32 device namespace: `\\.\PhysicalDrive0`, `\\.\COM1`. Never
    /// probed and never opened.
    Device,
    /// A network share. The payload is the host.
    Remote(String),
}

/// Classify `raw`'s UNC syntax.
///
/// A leading `\\` is Windows syntax wherever it appears, so it is always
/// classified. A leading `//` is only treated as UNC when this build runs on
/// Windows, because that is where Win32 normalises it into one — on macOS
/// `//Users/me` is a perfectly ordinary path, and the hazard being guarded
/// against (the local SMB client authenticating on a `stat`) is a property
/// of the machine doing the syscall, not of the path's spelling.
pub fn classify_unc(raw: &str) -> UncKind {
    let rest = match raw.strip_prefix(r"\\") {
        Some(r) => r,
        None => match raw.strip_prefix("//").filter(|_| cfg!(windows)) {
            Some(r) => r,
            None => return UncKind::Local,
        },
    };
    let head = rest.split(['\\', '/']).next().unwrap_or("");
    match head {
        "." => UncKind::Device,
        "?" => {
            let tail = &rest[head.len()..];
            let tail = tail.trim_start_matches(['\\', '/']);
            // `\\?\UNC\server\share` is a UNC path in verbatim clothing.
            if let Some(unc) = tail
                .strip_prefix("UNC\\")
                .or_else(|| tail.strip_prefix("UNC/"))
            {
                return match unc.split(['\\', '/']).next().unwrap_or("") {
                    "" => UncKind::Device,
                    host => UncKind::Remote(host.to_string()),
                };
            }
            // `\\?\C:\…` — a drive letter, colon, then a separator or end.
            let mut chars = tail.chars();
            match (chars.next(), chars.next(), chars.next()) {
                (Some(d), Some(':'), sep)
                    if d.is_ascii_alphabetic() && matches!(sep, None | Some('\\') | Some('/')) =>
                {
                    UncKind::VerbatimLocal
                }
                _ => UncKind::Device,
            }
        }
        "" => UncKind::Device,
        host => UncKind::Remote(host.to_string()),
    }
}

/// Hosts whose "network" paths never leave the machine, so probing them
/// cannot leak a credential or block on an unreachable server.
const LOCAL_UNC_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    // WSL's two spellings of the distro filesystem.
    "wsl$",
    "wsl.localhost",
];

fn same_host(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// May `raw` be stat'd automatically, on hover, with no click?
///
/// `cwd` is the pane's own working directory: if the user is already working
/// on `\\nas\projects`, the connection is open and authenticated and there
/// is nothing left to leak, so paths on that same host are fair game.
pub fn network_probe_allowed(raw: &str, cwd: Option<&str>) -> Result<(), String> {
    match classify_unc(raw) {
        UncKind::Local | UncKind::VerbatimLocal => Ok(()),
        UncKind::Device => Err("device-namespace paths are not resolved".into()),
        UncKind::Remote(host) => {
            if LOCAL_UNC_HOSTS.iter().any(|h| same_host(h, &host)) {
                return Ok(());
            }
            let cwd_host = cwd.map(classify_unc).and_then(|k| match k {
                UncKind::Remote(h) => Some(h),
                _ => None,
            });
            match cwd_host {
                Some(h) if same_host(&h, &host) => Ok(()),
                _ => Err(format!(
                    "network path on host {host:?} is not resolved automatically"
                )),
            }
        }
    }
}

/// Turn a raw candidate into the absolute path it names, or `None` when it
/// cannot be placed — a relative candidate with no cwd, or a `~` path on a
/// system with no home directory.
///
/// A rooted-but-driveless path on Windows (`/usr/lib`, `\srv\x`) is *not*
/// absolute to Rust, so it takes the `cwd` branch — where `Path::join`
/// keeps the cwd's drive and replaces the rest, giving `D:\usr\lib`. That
/// is Windows' own drive-relative rule, so it is the right answer, but it
/// means a POSIX path pasted into a Windows pane can resolve to a real
/// local file with a different meaning. Harmless in practice: the link only
/// appears when that file exists, the tooltip shows the absolute path that
/// was resolved, and a click opens that same path.
pub fn expand(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let tilde = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .map(Some)
        .or_else(|| if raw == "~" { Some(None) } else { None });

    let candidate = match tilde {
        Some(rest) => {
            let home = home?;
            match rest {
                Some(r) => home.join(r),
                None => home.to_path_buf(),
            }
        }
        None => PathBuf::from(raw),
    };

    if candidate.is_absolute() {
        return Some(candidate);
    }
    Some(cwd?.join(candidate))
}

/// Drop Windows' verbatim prefix from a canonicalised path.
///
/// `std::fs::canonicalize` returns `\\?\C:\x` and `\\?\UNC\host\share\x`.
/// Neither belongs in a tooltip, and `ShellExecuteW` does not reliably
/// accept the verbatim form, so both are rewritten to their plain spelling.
/// A no-op on every other platform and on paths that do not carry it.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Only the drive form is safe to unwrap; `\\?\Volume{…}` is not.
        let mut chars = rest.chars();
        if let (Some(d), Some(':')) = (chars.next(), chars.next()) {
            if d.is_ascii_alphabetic() {
                return PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// Extensions that `ShellExecuteW` or `open` would *run* rather than show.
///
/// Opening one of these on a click would turn "the terminal printed a path"
/// into "the terminal got code executed", with the user believing they asked
/// to look at a file. They are revealed in the file manager instead.
const RUNNABLE_EXTENSIONS: &[&str] = &[
    // Windows executables and installers
    "exe",
    "com",
    "scr",
    "pif",
    "msi",
    "msp",
    "msc",
    "cpl",
    "hta",
    "jar", //
    // Windows shells and script hosts
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "ps1xml",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "reg",
    // Shortcuts, which can point at anything
    "lnk",
    "url",
    "scf", //
    // macOS
    "app",
    "command",
    "workflow",
    "scpt",
    "applescript",
    "pkg",
    "mpkg",
    "term",
];

/// Should this path be revealed in the file manager instead of opened?
///
/// Directories are opened (that *is* "show it in the file manager"); files
/// whose extension would execute are revealed.
pub fn should_reveal(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            RUNNABLE_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Final gate before handing a path to the OS opener.
///
/// The path reaching here came from a probe that already vetted it, but it
/// arrives back over IPC, so it is checked again from scratch. Absoluteness
/// is load-bearing beyond tidiness: macOS's `open` takes flags, and a
/// relative candidate beginning with `-` would become one.
pub fn validate_open(raw: &str) -> Result<(), String> {
    if let Some(why) = reject_reason(raw) {
        return Err(why.to_string());
    }
    if matches!(classify_unc(raw), UncKind::Device) {
        return Err("device-namespace paths are not opened".into());
    }
    let path = Path::new(raw);
    // A UNC path is absolute on Windows but not when this check compiles for
    // a Unix target, so accept it explicitly rather than by platform.
    let unc = matches!(
        classify_unc(raw),
        UncKind::Remote(_) | UncKind::VerbatimLocal
    );
    if !path.is_absolute() && !unc {
        return Err("only absolute paths are opened".into());
    }
    Ok(())
}

/// Resolve one raw candidate against `cwd`, or `None` if it is not a path
/// worth linking. Touches the filesystem exactly once on the happy path
/// (plus a `canonicalize` for the display form).
pub fn probe_one(raw: &str, cwd: Option<&str>) -> Option<ResolvedPath> {
    if reject_reason(raw).is_some() {
        return None;
    }
    if network_probe_allowed(raw, cwd).is_err() {
        return None;
    }
    let expanded = expand(raw, cwd.map(Path::new), dirs::home_dir().as_deref())?;
    let meta = std::fs::metadata(&expanded).ok()?;
    let absolute = std::fs::canonicalize(&expanded)
        .map(|p| strip_verbatim(&p))
        .unwrap_or(expanded);
    Some(ResolvedPath {
        absolute: absolute.to_string_lossy().into_owned(),
        is_dir: meta.is_dir(),
    })
}

/// Resolve a batch, giving up on the whole batch after [`PROBE_TIMEOUT`].
///
/// The timeout runs on a detached worker rather than around each path: a
/// dead network drive blocks in `metadata` with no way to cancel it, so the
/// only thing that can be bounded is how long the *caller* waits. The output
/// always has exactly `raws.len()` entries so the frontend's index mapping
/// holds whatever happened.
pub fn probe_batch(raws: Vec<String>, cwd: Option<String>) -> Vec<Option<ResolvedPath>> {
    let want = raws.len();
    let batch: Vec<String> = raws.into_iter().take(MAX_BATCH).collect();
    let taken = batch.len();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let probed: Vec<Option<ResolvedPath>> = batch
            .iter()
            .map(|raw| probe_one(raw, cwd.as_deref()))
            .collect();
        // The receiver is gone on timeout; dropping the result is the point.
        let _ = tx.send(probed);
    });
    let mut out = rx
        .recv_timeout(PROBE_TIMEOUT)
        .unwrap_or_else(|_| vec![None; taken]);
    out.resize(want, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_webview_may_ask() {
        assert!(caller_allowed("main"));
        // Embedded browser panes carry `core:default` on http(s) origins, so
        // a page loaded in one can reach `invoke`.
        assert!(!caller_allowed("eb-1234"));
        assert!(!caller_allowed("eb-main"));
        assert!(!caller_allowed(""));
        assert!(!caller_allowed("Main"));
    }

    #[test]
    fn reject_reason_flags_control_characters() {
        assert!(reject_reason("src/main.ts").is_none());
        assert!(reject_reason("").is_some());
        assert!(reject_reason("a/\u{7}b").is_some());
        assert!(reject_reason("a/\nb").is_some());
        assert!(reject_reason("a/\u{1b}[0m").is_some());
        assert!(reject_reason(&"a/".repeat(MAX_RAW_LEN)).is_some());
    }

    #[test]
    fn reject_reason_allows_awkward_but_legal_names() {
        // All legal in a Windows or POSIX filename; none may be special to us
        // because nothing is ever handed to a shell.
        assert!(reject_reason(r"C:\a & b\c^d%e.txt").is_none());
        assert!(reject_reason("/srv/한글/문서.txt").is_none());
        assert!(reject_reason(r"/srv/it's a file").is_none());
    }

    #[test]
    fn classify_unc_reads_the_host() {
        assert_eq!(classify_unc(r"C:\Users\me"), UncKind::Local);
        assert_eq!(classify_unc("/usr/lib"), UncKind::Local);
        assert_eq!(classify_unc("src/main.ts"), UncKind::Local);
        assert_eq!(
            classify_unc(r"\\server\share\x"),
            UncKind::Remote("server".into())
        );
        assert_eq!(
            classify_unc(r"\\wsl$\Ubuntu\home\me"),
            UncKind::Remote("wsl$".into())
        );
    }

    #[test]
    fn classify_unc_rejects_the_device_namespace() {
        assert_eq!(classify_unc(r"\\.\PhysicalDrive0"), UncKind::Device);
        assert_eq!(classify_unc(r"\\.\COM1"), UncKind::Device);
        assert_eq!(classify_unc(r"\\"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\Volume{1234}\x"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\GLOBALROOT\Device\x"), UncKind::Device);
    }

    #[test]
    fn classify_unc_unwraps_the_verbatim_forms() {
        assert_eq!(classify_unc(r"\\?\C:\Users\me"), UncKind::VerbatimLocal);
        assert_eq!(classify_unc(r"\\?\c:"), UncKind::VerbatimLocal);
        assert_eq!(
            classify_unc(r"\\?\UNC\server\share\x"),
            UncKind::Remote("server".into())
        );
    }

    #[test]
    fn forward_slash_unc_is_windows_syntax_only() {
        // Win32 normalises `//host/share` into a UNC path; POSIX does not,
        // and `//Users/me` on macOS must stay an ordinary path.
        let kind = classify_unc("//server/share/x");
        if cfg!(windows) {
            assert_eq!(kind, UncKind::Remote("server".into()));
        } else {
            assert_eq!(kind, UncKind::Local);
        }
    }

    #[test]
    fn remote_hosts_are_not_probed_on_hover() {
        // Statting this on Windows opens SMB and offers NTLM credentials —
        // a credential leak triggered by a mouse move over terminal output.
        assert!(network_probe_allowed(r"\\evil.example\x\y", None).is_err());
        assert!(network_probe_allowed(r"\\10.0.0.9\share\x", Some(r"C:\repo")).is_err());
    }

    #[test]
    fn local_pseudo_hosts_are_probed() {
        assert!(network_probe_allowed(r"\\wsl$\Ubuntu\home\me", None).is_ok());
        assert!(network_probe_allowed(r"\\WSL.localhost\Ubuntu\etc", None).is_ok());
        assert!(network_probe_allowed(r"\\localhost\c$\Windows", None).is_ok());
    }

    #[test]
    fn the_panes_own_share_is_probed() {
        // Already connected and authenticated: nothing left to leak.
        assert!(network_probe_allowed(r"\\nas\projects\a.txt", Some(r"\\nas\projects")).is_ok());
        assert!(network_probe_allowed(r"\\NAS\projects\a.txt", Some(r"\\nas\other")).is_ok());
        assert!(network_probe_allowed(r"\\other\projects\a.txt", Some(r"\\nas\projects")).is_err());
    }

    #[test]
    fn ordinary_paths_are_always_probed() {
        assert!(network_probe_allowed(r"C:\repo\src\main.rs", None).is_ok());
        assert!(network_probe_allowed("/usr/lib/x", None).is_ok());
        assert!(network_probe_allowed("src/main.ts", Some("/repo")).is_ok());
    }

    #[test]
    fn expand_joins_a_relative_path_onto_the_cwd() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let got = expand("src/main.ts", Some(&cwd), None).unwrap();
        assert_eq!(got, cwd.join("src/main.ts"));
    }

    #[test]
    fn expand_refuses_a_relative_path_with_no_cwd() {
        assert!(expand("src/main.ts", None, None).is_none());
    }

    #[test]
    fn expand_leaves_an_absolute_path_alone() {
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert_eq!(expand(abs, None, None).unwrap(), PathBuf::from(abs));
    }

    #[test]
    #[cfg(windows)]
    fn expand_treats_a_driveless_root_as_drive_relative() {
        // Windows' own rule, and `Path::join` implements it: the cwd's drive
        // survives, everything after it is replaced.
        let cwd = PathBuf::from(r"D:\Git\ymux");
        assert_eq!(
            expand("/usr/lib", Some(&cwd), None).unwrap(),
            PathBuf::from("D:/usr/lib")
        );
        assert_eq!(
            expand(r"\srv\x", Some(&cwd), None).unwrap(),
            PathBuf::from(r"D:\srv\x")
        );
    }

    #[test]
    fn expand_substitutes_the_home_directory() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        });
        assert_eq!(
            expand("~/.claude/settings.json", None, Some(&home)).unwrap(),
            home.join(".claude/settings.json")
        );
        assert_eq!(expand("~", None, Some(&home)).unwrap(), home);
        assert!(expand("~/x", None, None).is_none());
    }

    #[test]
    fn expand_does_not_treat_a_tilde_name_as_home() {
        // `~foo` is shell syntax for another user's home, which this does not
        // implement; it must stay a literal relative name.
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let home = PathBuf::from("/home/me");
        assert_eq!(
            expand("~other/x", Some(&cwd), Some(&home)).unwrap(),
            cwd.join("~other/x")
        );
    }

    #[test]
    fn strip_verbatim_rewrites_both_canonical_forms() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\nas\projects\a.txt")),
            PathBuf::from(r"\\nas\projects\a.txt")
        );
    }

    #[test]
    fn strip_verbatim_leaves_everything_else_alone() {
        assert_eq!(
            strip_verbatim(Path::new("/usr/lib/x")),
            PathBuf::from("/usr/lib/x")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        // Not the drive form, so unwrapping it would change its meaning.
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\Volume{abc}\x")),
            PathBuf::from(r"\\?\Volume{abc}\x")
        );
    }

    #[test]
    fn executables_and_scripts_are_revealed_not_run() {
        for name in [
            "evil.bat",
            "evil.BAT",
            "setup.exe",
            "a.cmd",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.lnk",
            "a.url",
            "a.reg",
            "a.app",
            "a.command",
            "a.msi",
        ] {
            assert!(should_reveal(Path::new(name), false), "{name}");
        }
    }

    #[test]
    fn documents_and_directories_are_opened() {
        for name in [
            "notes.md",
            "a.rs",
            "a.ts",
            "photo.png",
            "report.pdf",
            "Makefile",
        ] {
            assert!(!should_reveal(Path::new(name), false), "{name}");
        }
        // A directory *is* the file-manager case, so it opens.
        assert!(!should_reveal(Path::new("scripts"), true));
        assert!(!should_reveal(Path::new("bundle.app"), true));
    }

    #[test]
    fn validate_open_requires_an_absolute_path() {
        // Relative names beginning with `-` would become flags to macOS `open`.
        assert!(validate_open("-R").is_err());
        assert!(validate_open("src/main.ts").is_err());
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert!(validate_open(abs).is_ok());
    }

    #[test]
    fn validate_open_accepts_a_unc_share_on_any_target() {
        // An explicit click is the consent the hover probe lacks, so a share
        // the user is genuinely working on stays openable.
        assert!(validate_open(r"\\nas\projects\a.txt").is_ok());
        assert!(validate_open(r"\\?\C:\x").is_ok());
    }

    #[test]
    fn validate_open_refuses_the_device_namespace_and_junk() {
        assert!(validate_open(r"\\.\PhysicalDrive0").is_err());
        assert!(validate_open("").is_err());
        assert!(validate_open("/x/\u{7}y").is_err());
    }

    #[test]
    fn probe_one_finds_a_real_file_and_directory() {
        let dir = std::env::temp_dir().join(format!("ymux-fspath-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.txt"), b"hi").unwrap();
        let cwd = dir.to_string_lossy().into_owned();

        let file = probe_one("sub/a.txt", Some(&cwd)).expect("relative file resolves");
        assert!(!file.is_dir);
        assert!(file.absolute.ends_with("a.txt"));
        // Absolute, so it can be opened without the cwd.
        assert!(Path::new(&file.absolute).is_absolute());

        let sub = probe_one("sub", Some(&cwd)).expect("relative dir resolves");
        assert!(sub.is_dir);

        assert!(probe_one("sub/missing.txt", Some(&cwd)).is_none());
        // No cwd, so a relative candidate cannot be placed at all.
        assert!(probe_one("sub/a.txt", None).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn probe_batch_answers_every_input_in_order() {
        let got = probe_batch(
            vec!["and/or".into(), "".into(), "n/a".into()],
            Some("/definitely/not/here".into()),
        );
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|r| r.is_none()));
    }

    #[test]
    fn probe_batch_pads_past_the_cap() {
        let raws: Vec<String> = (0..MAX_BATCH + 5).map(|i| format!("d{i}/f")).collect();
        let want = raws.len();
        assert_eq!(probe_batch(raws, None).len(), want);
    }
}
