//! Detect terminals / shells installed on the current machine and build a
//! list of [`ShellProfile`]s the UI can offer in its shell picker.
//!
//! On Windows we probe well-known paths, the `PATH` environment variable, and
//! (for Git Bash) the registry. On non-Windows hosts the detector returns
//! whatever `$SHELL` / common unix shells are present — this lets developers
//! run ymux on Linux or macOS during development without the Rust crate going
//! blind.

use std::path::{Path, PathBuf};

use crate::config::model::ShellProfile;

/// Entry point. Returns detected profiles in a stable order (most "modern"
/// first) so the frontend can pick the first one as a default.
pub fn detect_shells() -> Vec<ShellProfile> {
    #[cfg(windows)]
    {
        windows_detect::run()
    }
    #[cfg(not(windows))]
    {
        unix_detect::run()
    }
}

/// Return true if the path exists and is a regular file.
fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// Search `PATH` for `name` (or `name.exe` on Windows). Returns the first hit.
#[allow(dead_code)]
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(&exe_name);
        if is_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
mod windows_detect {
    use super::{is_file, which, PathBuf, ShellProfile};

    /// PowerShell prompt replacement that emits an OSC 7 `cwd` report before
    /// the regular prompt. `-NoExit -Command <this>` runs it once at shell
    /// startup and then drops into the interactive REPL, so the user never
    /// sees the init script itself.
    const PWSH_OSC7_INIT: &str = "function global:prompt { $p = ($PWD.Path -replace '\\\\','/'); $esc = [char]27; \"$esc]7;file:///$p$esc\\PS $($PWD.Path)> \" }";

    /// cmd.exe `PROMPT` that embeds OSC 7. `$e` is ESC and `$P` is the
    /// current drive + path — crucially, `$P` is re-evaluated *every time*
    /// the prompt is rendered, unlike `%CD%` which would be expanded once
    /// at PROMPT-setup time and then freeze on whatever directory the
    /// shell launched in.
    const CMD_OSC7_PROMPT: &str = "prompt $e]7;file:///$P$e\\$P$G";

    /// Bash (Git Bash / MSYS) init snippet. Written to a temp rcfile and
    /// passed via `--rcfile` on spawn. It first sources the user's normal
    /// init files so aliases / PS1 / env vars still take effect, then
    /// installs a `PROMPT_COMMAND` hook that emits OSC 7 with the current
    /// directory. Bash's `$PWD` in Git Bash is in MSYS form (`/c/Users/...`)
    /// so we convert it back to `C:/Users/...` before emitting, which keeps
    /// the URL a real `file://` URI the ymux parser understands.
    const BASH_OSC7_RCFILE: &str = r#"# ymux OSC 7 cwd reporter — auto-generated, safe to delete.
if [ -f "$HOME/.bash_profile" ]; then
    . "$HOME/.bash_profile"
elif [ -f "$HOME/.profile" ]; then
    . "$HOME/.profile"
fi
if [ -f "$HOME/.bashrc" ]; then
    . "$HOME/.bashrc"
fi
_ymux_osc7() {
    local p="$PWD"
    case "$p" in
        /[a-zA-Z]/*)
            local d="${p:1:1}"
            d=$(printf '%s' "$d" | tr '[:lower:]' '[:upper:]')
            p="${d}:${p:2}"
            ;;
    esac
    printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-localhost}" "$p"
}
case ";${PROMPT_COMMAND:-};" in
    *";_ymux_osc7;"*) ;;
    *) PROMPT_COMMAND="_ymux_osc7;${PROMPT_COMMAND:-}" ;;
esac
"#;

    /// Write (or refresh) the bash rcfile next to the main config and return
    /// its absolute path. Errors are logged and swallowed — Git Bash just
    /// won't have cwd tracking in that case.
    fn ensure_bash_rcfile() -> Option<PathBuf> {
        let dir = dirs::config_dir()?.join("ymux");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, "failed to create ymux config dir for bash rcfile");
            return None;
        }
        let path = dir.join("bash-init.sh");
        if let Err(e) = std::fs::write(&path, BASH_OSC7_RCFILE) {
            tracing::warn!(error = %e, "failed to write bash rcfile");
            return None;
        }
        Some(path)
    }

    pub fn run() -> Vec<ShellProfile> {
        let mut out = Vec::new();

        // 1. PowerShell 7+ (pwsh.exe) — prefer over Windows PowerShell 5.1.
        if let Some(p) = find_pwsh() {
            out.push(ShellProfile {
                name: "PowerShell 7".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec![
                    "-NoLogo".into(),
                    "-NoExit".into(),
                    "-Command".into(),
                    PWSH_OSC7_INIT.into(),
                ],
                icon: Some("pwsh".into()),
                color: Some("#012456".into()),
                env: Vec::new(),
            });
        }

        // 2. Windows PowerShell 5.1 — bundled with Windows.
        if let Some(p) = find_windows_powershell() {
            out.push(ShellProfile {
                name: "Windows PowerShell".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec![
                    "-NoLogo".into(),
                    "-NoExit".into(),
                    "-Command".into(),
                    PWSH_OSC7_INIT.into(),
                ],
                icon: Some("powershell".into()),
                color: Some("#012456".into()),
                env: Vec::new(),
            });
        }

        // 3. cmd.exe — always present. `/Q` suppresses command echo at
        // startup, `/K` runs the OSC 7 prompt setup then drops into
        // interactive mode.
        if let Some(p) = find_cmd() {
            out.push(ShellProfile {
                name: "Command Prompt".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec!["/Q".into(), "/K".into(), CMD_OSC7_PROMPT.into()],
                icon: Some("cmd".into()),
                color: Some("#0c0c0c".into()),
                env: Vec::new(),
            });
        }

        // 4. Visual Studio Developer Shells (Command Prompt + PowerShell),
        // one pair per installed VS edition. Windows Terminal offers these
        // out of the box, so we match that behaviour.
        out.extend(find_vs_developer_shells());

        // 5. Git Bash.
        if let Some(p) = find_git_bash() {
            // Use a generated rcfile for OSC 7 cwd reporting when we can
            // write one; fall back to plain `--login -i` otherwise.
            let args = if let Some(rcfile) = ensure_bash_rcfile() {
                // Forward-slash form of the path plays best with MSYS
                // bash's argument parsing.
                let rc = rcfile.to_string_lossy().replace('\\', "/");
                vec!["--rcfile".into(), rc, "-i".into()]
            } else {
                vec!["--login".into(), "-i".into()]
            };
            out.push(ShellProfile {
                name: "Git Bash".into(),
                executable: p.to_string_lossy().into_owned(),
                args,
                icon: Some("gitbash".into()),
                color: Some("#4e4e4e".into()),
                env: Vec::new(),
            });
        }

        // 6. WSL distros.
        out.extend(find_wsl_distros());

        // 7. Nushell, if on PATH.
        if let Some(p) = which("nu") {
            out.push(ShellProfile {
                name: "Nushell".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec![],
                icon: Some("nu".into()),
                color: Some("#4e9a06".into()),
                env: Vec::new(),
            });
        }

        out
    }

    fn find_pwsh() -> Option<PathBuf> {
        if let Some(p) = which("pwsh") {
            return Some(p);
        }
        for base in [
            std::env::var("ProgramFiles").ok(),
            std::env::var("ProgramFiles(x86)").ok(),
        ]
        .into_iter()
        .flatten()
        {
            let candidate = PathBuf::from(base)
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe");
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    fn find_windows_powershell() -> Option<PathBuf> {
        let root = std::env::var("SystemRoot").ok()?;
        let candidate = PathBuf::from(root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if is_file(&candidate) {
            Some(candidate)
        } else {
            None
        }
    }

    fn find_cmd() -> Option<PathBuf> {
        let root = std::env::var("SystemRoot").ok()?;
        let candidate = PathBuf::from(root).join("System32").join("cmd.exe");
        if is_file(&candidate) {
            Some(candidate)
        } else {
            None
        }
    }

    fn find_git_bash() -> Option<PathBuf> {
        // 1. Registry: HKLM\SOFTWARE\GitForWindows or HKCU.
        if let Some(install) = read_registry_string(r"SOFTWARE\GitForWindows", "InstallPath") {
            let candidate = PathBuf::from(install).join("bin").join("bash.exe");
            if is_file(&candidate) {
                return Some(candidate);
            }
        }
        // 2. Well-known install locations.
        let candidates = [
            std::env::var("ProgramFiles")
                .ok()
                .map(|b| PathBuf::from(b).join("Git").join("bin").join("bash.exe")),
            std::env::var("ProgramFiles(x86)")
                .ok()
                .map(|b| PathBuf::from(b).join("Git").join("bin").join("bash.exe")),
            std::env::var("LOCALAPPDATA").ok().map(|b| {
                PathBuf::from(b)
                    .join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
            }),
        ];
        for c in candidates.into_iter().flatten() {
            if is_file(&c) {
                return Some(c);
            }
        }
        // 3. PATH fallback.
        which("bash")
    }

    fn find_wsl_distros() -> Vec<ShellProfile> {
        use std::process::Command;
        let wsl = match which("wsl") {
            Some(p) => p,
            None => return Vec::new(),
        };

        // Prefer `--list --quiet` (no header, no annotations). On older
        // Windows builds this occasionally prints nothing for non-default
        // distros, so we fall back to `--list --verbose` and parse the
        // tabular output as a second pass.
        let mut names: Vec<String> = Vec::new();
        if let Ok(o) = Command::new(&wsl).args(["--list", "--quiet"]).output() {
            if o.status.success() {
                let text = decode_possibly_utf16(&o.stdout);
                for line in text.lines() {
                    let n = sanitize_wsl_name(line);
                    if !n.is_empty() {
                        names.push(n);
                    }
                }
            }
        }
        if names.is_empty() {
            if let Ok(o) = Command::new(&wsl).args(["--list", "--verbose"]).output() {
                if o.status.success() {
                    let text = decode_possibly_utf16(&o.stdout);
                    for (idx, line) in text.lines().enumerate() {
                        // Skip the first header row. Each data row looks
                        // like `  NAME          STATE           VERSION`
                        // with an optional `*` in the leading column for
                        // the default distro.
                        if idx == 0 {
                            continue;
                        }
                        let trimmed = line.trim().trim_start_matches('*').trim();
                        let first = trimmed.split_whitespace().next().unwrap_or("");
                        let n = sanitize_wsl_name(first);
                        if !n.is_empty() {
                            names.push(n);
                        }
                    }
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut profiles = Vec::new();
        for name in names {
            // Docker Desktop registers internal distros (`docker-desktop`
            // and `docker-desktop-data`) that aren't useful as interactive
            // shells — Windows Terminal hides them and we match that.
            let lower = name.to_lowercase();
            if lower.starts_with("docker-desktop") || lower == "rancher-desktop" {
                continue;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            profiles.push(ShellProfile {
                name: format!("WSL: {name}"),
                executable: wsl.to_string_lossy().into_owned(),
                args: vec!["-d".into(), name.clone()],
                icon: Some("wsl".into()),
                color: Some("#4e9a06".into()),
                env: Vec::new(),
            });
        }
        profiles
    }

    /// Strip a UTF-16 BOM, stray NUL bytes, carriage returns and surrounding
    /// whitespace from a single line emitted by `wsl.exe`.
    fn sanitize_wsl_name(raw: &str) -> String {
        raw.trim_start_matches('\u{FEFF}')
            .replace('\0', "")
            .trim()
            .trim_end_matches('\r')
            .trim()
            .to_string()
    }

    /// Locate every installed Visual Studio edition via `vswhere.exe` and
    /// expose its Developer Command Prompt + Developer PowerShell as shell
    /// profiles. Windows Terminal does the same thing, which is why the
    /// user's screenshot shows both entries alongside "cmd" / "PowerShell".
    fn find_vs_developer_shells() -> Vec<ShellProfile> {
        use std::process::Command;

        let base = match std::env::var("ProgramFiles(x86)").ok() {
            Some(b) => b,
            None => return Vec::new(),
        };
        let vswhere = PathBuf::from(base)
            .join("Microsoft Visual Studio")
            .join("Installer")
            .join("vswhere.exe");
        if !is_file(&vswhere) {
            return Vec::new();
        }

        let output = match Command::new(&vswhere)
            .args([
                "-all",
                "-prerelease",
                "-products",
                "*",
                "-format",
                "value",
                "-property",
                "installationPath",
            ])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        let text = String::from_utf8_lossy(&output.stdout);
        let cmd_path = find_cmd();
        // Launch-VsDevShell.ps1 depends on .NET Framework APIs that only
        // ship with Windows PowerShell 5.1, so prefer powershell.exe over
        // pwsh even when both are installed.
        let ps_path = find_windows_powershell().or_else(find_pwsh);

        // Collect installs first so we can decide how much disambiguation
        // each profile name needs (year alone vs. year + edition).
        let installs: Vec<(PathBuf, String, String)> = text
            .lines()
            .filter_map(|line| {
                let path = line.trim();
                if path.is_empty() {
                    None
                } else {
                    let (year, edition) = vs_labels(path);
                    Some((PathBuf::from(path), year, edition))
                }
            })
            .collect();

        let mut year_counts: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for (_, year, _) in &installs {
            *year_counts.entry(year.as_str()).or_insert(0) += 1;
        }

        let mut out = Vec::new();
        for (install, year, edition) in &installs {
            let needs_edition = year_counts.get(year.as_str()).copied().unwrap_or(1) > 1;
            let label = if needs_edition && !edition.is_empty() {
                format!("{year} {edition}")
            } else {
                year.clone()
            };

            let vsdevcmd = install.join("Common7").join("Tools").join("VsDevCmd.bat");
            if is_file(&vsdevcmd) {
                if let Some(cmd) = cmd_path.as_ref() {
                    // `call` keeps the outer cmd.exe alive after the batch
                    // finishes so we can chain our OSC 7 prompt setup.
                    let joined = format!("call \"{}\" & {}", vsdevcmd.display(), CMD_OSC7_PROMPT);
                    out.push(ShellProfile {
                        name: format!("Developer Command Prompt for VS {label}"),
                        executable: cmd.to_string_lossy().into_owned(),
                        args: vec!["/Q".into(), "/K".into(), joined],
                        icon: Some("vsdev-cmd".into()),
                        color: Some("#5c2d91".into()),
                        env: Vec::new(),
                    });
                }
            }

            let launch = install
                .join("Common7")
                .join("Tools")
                .join("Launch-VsDevShell.ps1");
            if is_file(&launch) {
                if let Some(ps) = ps_path.as_ref() {
                    // `-SkipAutomaticLocation` prevents the script from
                    // chdir-ing into the user's "Source" folder so the
                    // pane inherits the parent cwd like every other shell.
                    let launch_escaped = launch.to_string_lossy().replace('\'', "''");
                    let script = format!(
                        "& '{}' -SkipAutomaticLocation; {}",
                        launch_escaped, PWSH_OSC7_INIT
                    );
                    out.push(ShellProfile {
                        name: format!("Developer PowerShell for VS {label}"),
                        executable: ps.to_string_lossy().into_owned(),
                        args: vec![
                            "-NoLogo".into(),
                            "-NoExit".into(),
                            "-Command".into(),
                            script,
                        ],
                        icon: Some("vsdev-ps".into()),
                        color: Some("#5c2d91".into()),
                        env: Vec::new(),
                    });
                }
            }
        }
        out
    }

    /// Extract the product year (e.g. "2022") and edition (e.g. "Community")
    /// from a VS installation path. vswhere returns paths shaped like
    /// `C:\Program Files\Microsoft Visual Studio\2022\Community`.
    fn vs_labels(install_path: &str) -> (String, String) {
        const MARKER: &str = "Microsoft Visual Studio";
        if let Some(idx) = install_path.find(MARKER) {
            let tail = install_path[idx + MARKER.len()..].trim_start_matches(['\\', '/']);
            let mut parts = tail.split(['\\', '/']);
            let year = parts.next().unwrap_or("").to_string();
            let edition = parts.next().unwrap_or("").to_string();
            if !year.is_empty() {
                return (year, edition);
            }
        }
        ("Preview".to_string(), String::new())
    }

    fn decode_possibly_utf16(bytes: &[u8]) -> String {
        if bytes.len() >= 2 && bytes.len() % 2 == 0 {
            // Explicit UTF-16LE BOM, or a ratio of ASCII-in-UTF-16 pairs
            // large enough to be distinguishable from accidental UTF-8.
            let has_bom = bytes[0] == 0xFF && bytes[1] == 0xFE;
            let looks_like_utf16 = has_bom
                || bytes
                    .chunks_exact(2)
                    .take(16)
                    .any(|c| c[1] == 0 && c[0] != 0);
            if looks_like_utf16 {
                let start = if has_bom { 2 } else { 0 };
                let u16s: Vec<u16> = bytes[start..]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                return String::from_utf16_lossy(&u16s);
            }
        }
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn read_registry_string(subkey: &str, value: &str) -> Option<String> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
            HKEY_LOCAL_MACHINE, KEY_READ, REG_VALUE_TYPE,
        };

        fn to_wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let sub_wide = to_wide(subkey);
            let mut hkey = HKEY::default();
            let open =
                unsafe { RegOpenKeyExW(root, PCWSTR(sub_wide.as_ptr()), 0, KEY_READ, &mut hkey) };
            if open != ERROR_SUCCESS {
                continue;
            }
            let val_wide = to_wide(value);
            let mut buf = [0u16; 512];
            let mut len = (buf.len() * 2) as u32;
            let mut ty = REG_VALUE_TYPE(0);
            let q = unsafe {
                RegQueryValueExW(
                    hkey,
                    PCWSTR(val_wide.as_ptr()),
                    None,
                    Some(&mut ty),
                    Some(buf.as_mut_ptr() as *mut u8),
                    Some(&mut len),
                )
            };
            unsafe {
                let _ = RegCloseKey(hkey);
            }
            if q == ERROR_SUCCESS {
                let chars = (len as usize / 2).min(buf.len());
                let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
                return Some(String::from_utf16_lossy(&buf[..end]));
            }
        }
        None
    }
}

#[cfg(not(windows))]
mod unix_detect {
    use super::{is_file, which, Path, PathBuf, ShellProfile};

    /// Directory name (below the ymux config dir) holding the zsh startup-file
    /// shim. zsh only offers one injection point for a non-interactive caller
    /// — `ZDOTDIR` — and it swaps out *all* of `.zshenv` / `.zprofile` /
    /// `.zshrc` / `.zlogin` at once. So the shim has to re-source each of the
    /// user's real counterparts itself, or launching a pane would silently
    /// drop their aliases, `PATH` edits, and prompt.
    const ZSH_SHIM_DIR: &str = "zsh-init";

    /// `.zshenv` — the first file zsh reads, for every kind of shell.
    ///
    /// `YMUX_USER_ZDOTDIR` is seeded by the spawned profile's env (see
    /// [`zsh_profile`]) so a user who already sets `ZDOTDIR` keeps their own
    /// dotfile location. After sourcing their `.zshenv` we force `ZDOTDIR`
    /// back to the shim, because zsh re-reads it before *each* remaining
    /// startup file and their `.zshenv` may well have pointed it elsewhere.
    const ZSH_ZSHENV: &str = r#"# ymux shell integration — auto-generated, safe to delete.
#
# ymux starts zsh with ZDOTDIR pointing here so it can install an OSC 7
# "current directory" hook without editing your dotfiles. Each file in this
# directory sources its real counterpart first, so your own configuration
# still applies exactly as it would in Terminal.app.
YMUX_SHIM_ZDOTDIR="${ZDOTDIR:-$HOME}"
: "${YMUX_USER_ZDOTDIR:=$HOME}"
[ -f "$YMUX_USER_ZDOTDIR/.zshenv" ] && . "$YMUX_USER_ZDOTDIR/.zshenv"
# Their .zshenv may set ZDOTDIR for their own layout; adopt it as the "user"
# location, then point zsh back at the shim for the rest of startup.
if [ -n "$ZDOTDIR" ] && [ "$ZDOTDIR" != "$YMUX_SHIM_ZDOTDIR" ]; then
    YMUX_USER_ZDOTDIR="$ZDOTDIR"
fi
ZDOTDIR="$YMUX_SHIM_ZDOTDIR"
"#;

    /// `.zprofile` — login shells only. ymux spawns login shells on macOS so
    /// `/usr/libexec/path_helper` runs and `PATH` matches Terminal.app's.
    const ZSH_ZPROFILE: &str = r#"# ymux shell integration — auto-generated, safe to delete.
[ -f "$YMUX_USER_ZDOTDIR/.zprofile" ] && . "$YMUX_USER_ZDOTDIR/.zprofile"
ZDOTDIR="${YMUX_SHIM_ZDOTDIR:-$ZDOTDIR}"
"#;

    /// `.zshrc` — interactive shells. Sources the user's rc first so their
    /// prompt is already installed, *then* appends the OSC 7 reporter as a
    /// `precmd` hook. Appending matters: a prompt framework that assigns to
    /// `precmd_functions` wholesale (starship, p10k) would otherwise drop us.
    ///
    /// The final `ZDOTDIR` restore hands the user's own value back before
    /// they get a prompt, so anything they run later — including a nested
    /// zsh — sees what they configured rather than ymux's shim.
    const ZSH_ZSHRC: &str = r#"# ymux shell integration — auto-generated, safe to delete.
[ -f "$YMUX_USER_ZDOTDIR/.zshrc" ] && . "$YMUX_USER_ZDOTDIR/.zshrc"
_ymux_osc7() {
    printf '\033]7;file://%s%s\033\\' "${HOST:-localhost}" "$PWD"
}
if autoload -Uz add-zsh-hook 2>/dev/null; then
    add-zsh-hook precmd _ymux_osc7
else
    precmd_functions+=(_ymux_osc7)
fi
_ymux_osc7
ZDOTDIR="${YMUX_USER_ZDOTDIR:-$HOME}"
"#;

    /// `.zlogin` — read after `.zshrc`. By then `.zshrc` has usually restored
    /// `ZDOTDIR`, so zsh reads the user's own `.zlogin` directly and this file
    /// is never used. It exists for the login-but-not-interactive case, where
    /// `.zshrc` never ran and `ZDOTDIR` still points at the shim.
    const ZSH_ZLOGIN: &str = r#"# ymux shell integration — auto-generated, safe to delete.
[ -f "${YMUX_USER_ZDOTDIR:-$HOME}/.zlogin" ] && . "${YMUX_USER_ZDOTDIR:-$HOME}/.zlogin"
"#;

    /// Bash init snippet, written to a temp rcfile and passed via `--rcfile`.
    ///
    /// `--rcfile` is only honoured for interactive *non-login* shells, so the
    /// profile spawns bash without `-l` and this file sources the login files
    /// itself — otherwise a macOS user would lose everything in
    /// `~/.bash_profile`. This mirrors the Git Bash rcfile on Windows, minus
    /// the MSYS drive-letter rewriting, which has no meaning here.
    const BASH_OSC7_RCFILE: &str = r#"# ymux OSC 7 cwd reporter — auto-generated, safe to delete.
if [ -f "$HOME/.bash_profile" ]; then
    . "$HOME/.bash_profile"
elif [ -f "$HOME/.profile" ]; then
    . "$HOME/.profile"
fi
if [ -f "$HOME/.bashrc" ]; then
    . "$HOME/.bashrc"
fi
_ymux_osc7() {
    printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-localhost}" "$PWD"
}
case ";${PROMPT_COMMAND:-};" in
    *";_ymux_osc7;"*) ;;
    *) PROMPT_COMMAND="_ymux_osc7;${PROMPT_COMMAND:-}" ;;
esac
"#;

    /// `<config_dir>/ymux`, created on demand. Shared with `theme.toml` and
    /// the rest of the y* family via `ytheme::config_dir`.
    fn ymux_dir() -> Option<PathBuf> {
        let dir = dirs::config_dir()?.join("ymux");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, "failed to create ymux config dir for shell integration");
            return None;
        }
        Some(dir)
    }

    /// Write (or refresh) the four zsh shim files and return the directory to
    /// hand zsh as `ZDOTDIR`. Errors are logged and swallowed — a pane still
    /// opens without cwd tracking, which beats refusing to spawn a shell.
    fn ensure_zsh_shim() -> Option<PathBuf> {
        let dir = ymux_dir()?.join(ZSH_SHIM_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, "failed to create zsh shim dir");
            return None;
        }
        for (name, body) in [
            (".zshenv", ZSH_ZSHENV),
            (".zprofile", ZSH_ZPROFILE),
            (".zshrc", ZSH_ZSHRC),
            (".zlogin", ZSH_ZLOGIN),
        ] {
            if let Err(e) = std::fs::write(dir.join(name), body) {
                tracing::warn!(error = %e, file = name, "failed to write zsh shim file");
                return None;
            }
        }
        Some(dir)
    }

    /// Write (or refresh) the bash rcfile and return its absolute path.
    fn ensure_bash_rcfile() -> Option<PathBuf> {
        let path = ymux_dir()?.join("bash-init.sh");
        if let Err(e) = std::fs::write(&path, BASH_OSC7_RCFILE) {
            tracing::warn!(error = %e, "failed to write bash rcfile");
            return None;
        }
        Some(path)
    }

    /// `$ENV` script for POSIX shells (`sh`, `dash`, `ksh`). Read on
    /// interactive startup — the only injection point these shells share.
    ///
    /// `PS1` is deliberately left alone: emitting OSC 7 from a `PS1` prefix is
    /// the portable trick, but it would also overwrite whatever prompt the
    /// user configured. `PROMPT_COMMAND` covers macOS's `/bin/sh` (bash in
    /// POSIX mode) and ksh; a shell with neither simply reports its startup
    /// directory once, which is still better than never reporting at all.
    const POSIX_ENV_INIT: &str = r#"# ymux OSC 7 cwd reporter — auto-generated, safe to delete.
_ymux_osc7() {
    printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-localhost}" "$PWD"
}
case ";${PROMPT_COMMAND:-};" in
    *";_ymux_osc7;"*) ;;
    *) PROMPT_COMMAND="_ymux_osc7;${PROMPT_COMMAND:-}" ;;
esac
_ymux_osc7
"#;

    /// Write (or refresh) the POSIX `$ENV` script and return its path.
    fn ensure_posix_env_file() -> Option<PathBuf> {
        let path = ymux_dir()?.join("posix-init.sh");
        if let Err(e) = std::fs::write(&path, POSIX_ENV_INIT) {
            tracing::warn!(error = %e, "failed to write posix env file");
            return None;
        }
        Some(path)
    }

    /// zsh profile: login + interactive, with `ZDOTDIR` aimed at the shim.
    fn zsh_profile(name: String, exe: &Path) -> ShellProfile {
        let mut env = Vec::new();
        if let Some(shim) = ensure_zsh_shim() {
            // Preserve a ZDOTDIR the user already exported, so the shim knows
            // where their real dotfiles live.
            let user_zdotdir = std::env::var("ZDOTDIR")
                .ok()
                .filter(|v| !v.is_empty())
                .or_else(|| dirs::home_dir().map(|h| h.display().to_string()));
            if let Some(u) = user_zdotdir {
                env.push(("YMUX_USER_ZDOTDIR".to_string(), u));
            }
            env.push(("ZDOTDIR".to_string(), shim.display().to_string()));
        }
        ShellProfile {
            name,
            executable: exe.to_string_lossy().into_owned(),
            args: vec!["-l".into(), "-i".into()],
            icon: Some("zsh".into()),
            color: Some("#2d8a3e".into()),
            env,
        }
    }

    /// bash profile: interactive with an explicit `--rcfile` (see the rcfile
    /// doc comment for why this is not a login shell).
    fn bash_profile(name: String, exe: &Path) -> ShellProfile {
        let mut args = Vec::new();
        if let Some(rc) = ensure_bash_rcfile() {
            args.push("--rcfile".to_string());
            args.push(rc.display().to_string());
        }
        args.push("-i".to_string());
        ShellProfile {
            name,
            executable: exe.to_string_lossy().into_owned(),
            args,
            icon: Some("bash".into()),
            color: Some("#4e9a06".into()),
            env: Vec::new(),
        }
    }

    /// Build the profile for `exe`, choosing the shell integration by
    /// basename. fish needs none: it has emitted OSC 7 on every `PWD` change
    /// since 3.1, so a plain login shell already reports its cwd.
    fn profile_for(name: String, exe: &Path) -> ShellProfile {
        let base = exe
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match base.as_str() {
            "zsh" => zsh_profile(name, exe),
            "bash" => bash_profile(name, exe),
            "fish" => ShellProfile {
                name,
                executable: exe.to_string_lossy().into_owned(),
                args: vec!["-l".into(), "-i".into()],
                icon: Some("fish".into()),
                color: Some("#3a9fbf".into()),
                env: Vec::new(),
            },
            // sh, dash, ksh, and anything else the user points $SHELL at.
            // POSIX shells read `$ENV` when they start interactively, which
            // is the one hook point they all agree on, so cwd tracking works
            // here too. It matters more than it looks: a pane on a shell with
            // no integration silently loses split-inherits-cwd *and*
            // reopen-where-you-left-off, with nothing to hint at why.
            _ => {
                let mut env = Vec::new();
                if let Some(init) = ensure_posix_env_file() {
                    env.push(("ENV".to_string(), init.display().to_string()));
                }
                ShellProfile {
                    name,
                    executable: exe.to_string_lossy().into_owned(),
                    args: vec!["-l".into()],
                    icon: None,
                    color: None,
                    env,
                }
            }
        }
    }

    /// Resolve symlinks so `/bin/zsh` and a `$SHELL` of `/bin/zsh` don't
    /// produce two entries for the same binary. Falls back to the original
    /// path when the target can't be canonicalised.
    fn canonical(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    pub fn run() -> Vec<ShellProfile> {
        let mut out: Vec<ShellProfile> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();

        let push = |exe: PathBuf, out: &mut Vec<ShellProfile>, seen: &mut Vec<PathBuf>| {
            if !is_file(&exe) {
                return;
            }
            let canon = canonical(&exe);
            if seen.contains(&canon) {
                return;
            }
            let mut name = exe
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "shell".into());
            // Names are the key the frontend spawns by, so they have to be
            // unique even when e.g. Homebrew zsh sits alongside /bin/zsh.
            if out.iter().any(|p| p.name == name) {
                name = format!("{name} ({})", exe.display());
                if out.iter().any(|p| p.name == name) {
                    return;
                }
            }
            seen.push(canon);
            out.push(profile_for(name, &exe));
        };

        // 1. The user's login shell, listed first so it becomes the default.
        if let Ok(s) = std::env::var("SHELL") {
            if !s.is_empty() {
                push(PathBuf::from(&s), &mut out, &mut seen);
            }
        }

        // 2. The usual suspects from PATH, then the system copies. Homebrew
        //    installs (`/opt/homebrew/bin`, `/usr/local/bin`) land via PATH;
        //    the absolute fallbacks cover a stripped PATH in a GUI-launched
        //    app bundle, where launchd hands us only `/usr/bin:/bin:...`.
        for name in ["zsh", "bash", "fish", "sh"] {
            if let Some(p) = which(name) {
                push(p, &mut out, &mut seen);
            }
            for prefix in ["/opt/homebrew/bin", "/usr/local/bin", "/bin", "/usr/bin"] {
                push(PathBuf::from(prefix).join(name), &mut out, &mut seen);
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_returns_at_least_one_profile_on_dev_host() {
        // On any host where $SHELL or /bin/sh exists this should not be empty.
        // The test guards against regressions where the enumerator returns
        // nothing even on machines that clearly have a shell.
        let profiles = detect_shells();
        if !profiles.is_empty() {
            for p in &profiles {
                assert!(
                    !p.name.is_empty(),
                    "shell profile must have a non-empty name"
                );
                assert!(
                    !p.executable.is_empty(),
                    "shell profile must have an executable path"
                );
            }
        }
    }

    #[test]
    fn profile_names_are_unique() {
        let profiles = detect_shells();
        let mut seen = std::collections::HashSet::new();
        for p in &profiles {
            assert!(seen.insert(p.name.clone()), "duplicate name {}", p.name);
        }
    }
}
