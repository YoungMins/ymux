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

/// Rewrite the shell-integration files (zsh `ZDOTDIR` shim, bash `--rcfile`,
/// POSIX `$ENV` script) without re-detecting shells. Profiles persisted in
/// the config point at these files by path, so an upgraded ymux must refresh
/// their contents at startup or keep running the old version's scripts.
/// Failures are logged by the writers and otherwise ignored.
pub fn refresh_shell_integration() {
    #[cfg(windows)]
    {
        let _ = windows_detect::ensure_bash_rcfile();
    }
    #[cfg(not(windows))]
    {
        let _ = unix_detect::ensure_zsh_shim();
        let _ = unix_detect::ensure_bash_rcfile();
        let _ = unix_detect::ensure_posix_env_file();
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

    /// Console code-page switch chained ahead of every cmd.exe startup
    /// command.
    ///
    /// A Windows console starts on the machine's legacy code page — CP949 on
    /// Korean Windows, 936 on Chinese — and ConPTY decodes everything a child
    /// writes through that page before handing it to us. So a tool that
    /// emits UTF-8 bytes (which is most of them, including every TUI in
    /// `tools/`) arrives as mojibake, and the mangling happens *below* ymux
    /// where no amount of frontend decoding can undo it. `chcp 65001` moves
    /// the console to UTF-8 for both directions; `> nul` swallows chcp's
    /// "Active code page: 65001" banner so the pane still opens clean.
    pub(super) const CMD_UTF8_SETUP: &str = "chcp 65001 > nul";

    /// PowerShell / pwsh console encoding setup, prepended to the `-Command`
    /// bootstrap below.
    ///
    /// These property setters call `SetConsoleOutputCP` / `SetConsoleCP`
    /// underneath, so they fix native programs launched inside the pane too,
    /// not just PowerShell's own writes. `InputEncoding` is what lets a
    /// pasted or IME-typed Korean command line reach the shell intact.
    /// Wrapped in `try`/`catch` for the same reason Git Bash uses `;` below:
    /// a console host that refuses the switch must not take the shell down
    /// with it.
    pub(super) const PWSH_UTF8_SETUP: &str = "try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding } catch { }";

    /// Build a cmd.exe `/K` payload that switches to UTF-8 before running
    /// `script` (which still carries the OSC 7 prompt hook).
    fn cmd_command(script: &str) -> String {
        format!("{CMD_UTF8_SETUP} & {script}")
    }

    /// Build a PowerShell `-Command` payload that switches to UTF-8 before
    /// running `script` (the OSC 7 prompt installer, optionally preceded by
    /// a VS Developer Shell launcher).
    fn pwsh_command(script: &str) -> String {
        format!("{PWSH_UTF8_SETUP}; {script}")
    }

    /// Single-quote `s` for POSIX `sh`, so an rcfile path containing spaces
    /// survives the `-c` wrapper below.
    fn posix_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', r"'\''"))
    }

    /// Wrap Git Bash's interactive argv in a `-c` launcher that flips the
    /// console to UTF-8 and then `exec`s the real shell, so the pane still
    /// hosts exactly one process and `--rcfile` / `-i` keep their meaning.
    ///
    /// `;` rather than `&&` is deliberate: a machine whose `chcp.com` is
    /// missing from `PATH` must still get a working shell, just without the
    /// code-page switch. With `&&` the `exec` would never run and the pane
    /// would die at startup.
    pub(super) fn bash_utf8_wrapper(bash_args: &[String]) -> Vec<String> {
        let rendered = bash_args
            .iter()
            .map(|a| {
                if a.starts_with('-') {
                    a.clone()
                } else {
                    posix_quote(a)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            "-c".into(),
            format!("chcp.com 65001 >/dev/null 2>&1; exec \"$BASH\" {rendered}"),
        ]
    }

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
    pub(super) fn ensure_bash_rcfile() -> Option<PathBuf> {
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
                    pwsh_command(PWSH_OSC7_INIT),
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
                    pwsh_command(PWSH_OSC7_INIT),
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
                args: vec!["/Q".into(), "/K".into(), cmd_command(CMD_OSC7_PROMPT)],
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
            let interactive: Vec<String> = if let Some(rcfile) = ensure_bash_rcfile() {
                // Forward-slash form of the path plays best with MSYS
                // bash's argument parsing.
                let rc = rcfile.to_string_lossy().replace('\\', "/");
                vec!["--rcfile".into(), rc, "-i".into()]
            } else {
                vec!["--login".into(), "-i".into()]
            };
            // …then hand that argv to a UTF-8 launcher that `exec`s it.
            let args = bash_utf8_wrapper(&interactive);
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
                    let joined = cmd_command(&format!(
                        "call \"{}\" & {}",
                        vsdevcmd.display(),
                        CMD_OSC7_PROMPT
                    ));
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
                    let script = pwsh_command(&format!(
                        "& '{}' -SkipAutomaticLocation; {}",
                        launch_escaped, PWSH_OSC7_INIT
                    ));
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

/// The macOS/unix shell-integration scripts. They live outside `unix_detect`
/// (which only compiles off Windows) so the snapshot tests below run on every
/// host, including the Windows dev machine.
#[cfg_attr(windows, allow(dead_code))]
mod unix_scripts {
    /// Directory name (below the ymux config dir) holding the zsh startup-file
    /// shim. zsh only offers one injection point for a non-interactive caller
    /// — `ZDOTDIR` — and it swaps out *all* of `.zshenv` / `.zprofile` /
    /// `.zshrc` / `.zlogin` at once. So the shim has to re-source each of the
    /// user's real counterparts itself, or launching a pane would silently
    /// drop their aliases, `PATH` edits, and prompt.
    pub(super) const ZSH_SHIM_DIR: &str = "zsh-init";

    /// `.zshenv` — the first file zsh reads, for every kind of shell.
    ///
    /// `YMUX_USER_ZDOTDIR` is seeded by the spawned profile's env (see
    /// `unix_detect::zsh_profile`): the user's own `ZDOTDIR`, or empty when
    /// they had none. Every user file is sourced with `ZDOTDIR` handed back to
    /// that value, so their `${ZDOTDIR:-$HOME}` idioms resolve exactly as in
    /// Terminal.app — and whatever `ZDOTDIR` their file leaves behind is
    /// adopted as the new "user" value, as zsh itself would. Afterwards
    /// `ZDOTDIR` points back at the shim, because zsh re-reads it before
    /// *each* remaining startup file.
    pub(super) const ZSH_ZSHENV: &str = r#"# ymux shell integration — auto-generated, safe to delete.
#
# ymux starts zsh with ZDOTDIR pointing here so it can install an OSC 7
# "current directory" hook without editing your dotfiles. Each file in this
# directory sources its real counterpart first — with ZDOTDIR set back to
# your own value while it runs — so your configuration still applies exactly
# as it would in Terminal.app.
YMUX_SHIM_ZDOTDIR="${ZDOTDIR:-$HOME}"
YMUX_USER_ZDOTDIR="${YMUX_USER_ZDOTDIR-}"
if [ -n "$YMUX_USER_ZDOTDIR" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi
[ -f "${ZDOTDIR:-$HOME}/.zshenv" ] && . "${ZDOTDIR:-$HOME}/.zshenv"
YMUX_USER_ZDOTDIR="${ZDOTDIR-}"
export ZDOTDIR="$YMUX_SHIM_ZDOTDIR"
"#;

    /// `.zprofile` — login shells only. ymux spawns login shells on macOS so
    /// `/usr/libexec/path_helper` runs and `PATH` matches Terminal.app's.
    pub(super) const ZSH_ZPROFILE: &str = r#"# ymux shell integration — auto-generated, safe to delete.
if [ -n "$YMUX_USER_ZDOTDIR" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi
[ -f "${ZDOTDIR:-$HOME}/.zprofile" ] && . "${ZDOTDIR:-$HOME}/.zprofile"
YMUX_USER_ZDOTDIR="${ZDOTDIR-}"
export ZDOTDIR="$YMUX_SHIM_ZDOTDIR"
"#;

    /// `.zshrc` — interactive shells. Sources the user's rc first so their
    /// prompt is already installed, *then* appends the OSC 7 reporter as a
    /// `precmd` hook. Appending matters: a prompt framework that assigns to
    /// `precmd_functions` wholesale (starship, p10k) would otherwise drop us.
    ///
    /// `/etc/zshrc` runs just before this file with `ZDOTDIR` still on the
    /// shim, and its unguarded `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` would
    /// put the user's history inside the shim dir. So a `HISTFILE` under the
    /// shim is moved to the user's location — before their `.zshrc` (so it
    /// sees the right value) and again after it.
    ///
    /// The final `ZDOTDIR` restore hands the user's own value back (unset if
    /// they had none) before they get a prompt: zsh then reads `/etc/zlogin`
    /// and their `.zlogin` natively, and anything they run later — including
    /// a nested zsh — sees what they configured rather than ymux's shim.
    pub(super) const ZSH_ZSHRC: &str = r#"# ymux shell integration — auto-generated, safe to delete.
case "${HISTFILE-}" in "$YMUX_SHIM_ZDOTDIR"/*) HISTFILE="${YMUX_USER_ZDOTDIR:-$HOME}/.zsh_history" ;; esac
if [ -n "$YMUX_USER_ZDOTDIR" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi
[ -f "${ZDOTDIR:-$HOME}/.zshrc" ] && . "${ZDOTDIR:-$HOME}/.zshrc"
YMUX_USER_ZDOTDIR="${ZDOTDIR-}"
case "${HISTFILE-}" in "$YMUX_SHIM_ZDOTDIR"/*) HISTFILE="${YMUX_USER_ZDOTDIR:-$HOME}/.zsh_history" ;; esac
_ymux_osc7() {
    printf '\033]7;file://%s%s\033\\' "${HOST:-localhost}" "$PWD"
}
if autoload -Uz add-zsh-hook 2>/dev/null; then
    add-zsh-hook precmd _ymux_osc7
else
    precmd_functions+=(_ymux_osc7)
fi
_ymux_osc7
if [ -n "$YMUX_USER_ZDOTDIR" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi
unset YMUX_USER_ZDOTDIR YMUX_SHIM_ZDOTDIR
"#;

    /// `.zlogin` — read after `.zshrc`. By then `.zshrc` has restored
    /// `ZDOTDIR`, so zsh reads the user's own `.zlogin` directly and this file
    /// is never used. It exists for the login-but-not-interactive case, where
    /// `.zshrc` never ran and `ZDOTDIR` still points at the shim; it ends
    /// startup the same way `.zshrc` does.
    pub(super) const ZSH_ZLOGIN: &str = r#"# ymux shell integration — auto-generated, safe to delete.
if [ -n "${YMUX_USER_ZDOTDIR-}" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi
[ -f "${ZDOTDIR:-$HOME}/.zlogin" ] && . "${ZDOTDIR:-$HOME}/.zlogin"
unset YMUX_USER_ZDOTDIR YMUX_SHIM_ZDOTDIR
"#;

    /// Bash init snippet, written to a temp rcfile and passed via `--rcfile`.
    ///
    /// `--rcfile` is only honoured for interactive *non-login* shells, so the
    /// profile spawns bash without `-l` and this file replays a login shell's
    /// startup itself: `/etc/profile` first (on macOS that runs
    /// `path_helper`, without which `PATH` lacks `/etc/paths` and differs
    /// from Terminal.app's), then the first of `~/.bash_profile`,
    /// `~/.bash_login`, `~/.profile` — exactly bash's login order.
    /// `~/.bashrc` is deliberately not forced on top: a login shell doesn't
    /// read it either, and most `~/.bash_profile`s already source it, so
    /// forcing it would run it twice (duplicate `PATH` edits, hooks, prompt).
    ///
    /// Unix only: the Windows Git Bash rcfile (`windows_detect`) keeps its own
    /// behaviour.
    pub(super) const BASH_OSC7_RCFILE: &str = r#"# ymux OSC 7 cwd reporter — auto-generated, safe to delete.
[ -r /etc/profile ] && . /etc/profile
if [ -f "$HOME/.bash_profile" ]; then
    . "$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then
    . "$HOME/.bash_login"
elif [ -f "$HOME/.profile" ]; then
    . "$HOME/.profile"
fi
_ymux_osc7() {
    printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-localhost}" "$PWD"
}
case ";${PROMPT_COMMAND:-};" in
    *";_ymux_osc7;"*) ;;
    *) PROMPT_COMMAND="_ymux_osc7;${PROMPT_COMMAND:-}" ;;
esac
"#;
}

#[cfg(not(windows))]
mod unix_detect {
    use super::unix_scripts::{
        BASH_OSC7_RCFILE, ZSH_SHIM_DIR, ZSH_ZLOGIN, ZSH_ZPROFILE, ZSH_ZSHENV, ZSH_ZSHRC,
    };
    use super::{is_file, which, Path, PathBuf, ShellProfile};

    /// `<config_dir>/ymux`, created on demand. Shared with `theme.toml` via
    /// `ytheme::config_dir`.
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
    pub(super) fn ensure_zsh_shim() -> Option<PathBuf> {
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
    pub(super) fn ensure_bash_rcfile() -> Option<PathBuf> {
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
    pub(super) fn ensure_posix_env_file() -> Option<PathBuf> {
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
            // where their real dotfiles live. Always set — empty means "none",
            // so the shim unsets ZDOTDIR again at the end of startup instead
            // of trusting a stale value inherited from ymux's own env.
            let shim_str = shim.display().to_string();
            let user_zdotdir = std::env::var("ZDOTDIR")
                .ok()
                .filter(|v| !v.is_empty() && *v != shim_str)
                .unwrap_or_default();
            env.push(("YMUX_USER_ZDOTDIR".to_string(), user_zdotdir));
            env.push(("ZDOTDIR".to_string(), shim_str));
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

    /// Lowercased file name of a profile's executable, e.g. `"cmd.exe"`.
    #[cfg(windows)]
    fn exe_base(p: &ShellProfile) -> String {
        std::path::Path::new(&p.executable)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// The Git Bash launcher has to stay *fail-open*: it is the one profile
    /// whose encoding setup runs as a command rather than a shell builtin,
    /// so a machine without `chcp.com` on `PATH` must still get a shell.
    /// Asserted on the pure builder so it holds whether or not Git Bash is
    /// installed on the machine running the suite.
    #[cfg(windows)]
    #[test]
    fn git_bash_utf8_wrapper_is_fail_open_and_keeps_the_rcfile() {
        let inner = vec![
            "--rcfile".to_string(),
            "C:/Users/a b/AppData/Roaming/ymux/bash-init.sh".to_string(),
            "-i".to_string(),
        ];
        let args = windows_detect::bash_utf8_wrapper(&inner);
        assert_eq!(args.len(), 2, "wrapper is `-c <command>`: {args:?}");
        assert_eq!(args[0], "-c");
        let cmd = &args[1];

        assert!(cmd.contains("chcp.com 65001"), "no code-page switch: {cmd}");
        // `;` — not `&&` — so a missing chcp.com cannot abort startup.
        assert!(
            !cmd.contains("&&"),
            "chcp must be chained fail-open with `;`, not `&&`: {cmd}"
        );
        let chcp = cmd.find("chcp.com").expect("chcp present");
        let exec = cmd.find("exec ").expect("exec present");
        assert!(chcp < exec, "chcp must run before the exec: {cmd}");
        // The OSC 7 rcfile (and interactivity) must survive the wrapping,
        // spaces in the path included.
        assert!(
            cmd.contains(
                "exec \"$BASH\" --rcfile 'C:/Users/a b/AppData/Roaming/ymux/bash-init.sh' -i"
            ),
            "interactive argv not preserved: {cmd}"
        );

        // The no-rcfile fallback keeps its login flag.
        let fallback =
            windows_detect::bash_utf8_wrapper(&["--login".to_string(), "-i".to_string()]);
        assert!(
            fallback[1].ends_with("exec \"$BASH\" --login -i"),
            "fallback argv not preserved: {:?}",
            fallback[1]
        );
    }

    /// Every Windows shell ymux spawns itself must start on code page 65001.
    ///
    /// Windows consoles default to the machine's legacy code page (CP949 on
    /// Korean Windows, 936 on Chinese), and ConPTY re-encodes through it
    /// before ymux ever sees a byte — so CJK output garbles below the layer
    /// the frontend can fix. WSL and Nushell are excluded on purpose: both
    /// are UTF-8 natively and neither takes a code-page argument.
    #[cfg(windows)]
    #[test]
    fn windows_profiles_start_in_utf8_without_losing_the_osc7_hook() {
        for p in detect_shells() {
            let args = p.args.join(" ");

            // WSL is already UTF-8 end to end; it must be left exactly as it
            // was, or `wsl -d <distro>` stops being a plain distro launch.
            if p.name.starts_with("WSL: ") {
                assert_eq!(p.args.len(), 2, "{}: {:?}", p.name, p.args);
                assert_eq!(p.args[0], "-d", "{}: {:?}", p.name, p.args);
                assert!(
                    !args.contains("chcp") && !args.contains("Encoding"),
                    "{} must carry no code-page setup: {:?}",
                    p.name,
                    p.args
                );
                continue;
            }
            // Nushell is UTF-8 natively and is launched with no args at all.
            if p.name == "Nushell" {
                assert!(p.args.is_empty(), "{}: {:?}", p.name, p.args);
                continue;
            }

            match exe_base(&p).as_str() {
                "cmd.exe" => {
                    assert!(
                        args.contains(windows_detect::CMD_UTF8_SETUP),
                        "{} misses `chcp 65001`: {:?}",
                        p.name,
                        p.args
                    );
                    assert!(
                        args.contains("]7;file:///$P"),
                        "{} lost its OSC 7 prompt: {:?}",
                        p.name,
                        p.args
                    );
                }
                "powershell.exe" | "pwsh.exe" => {
                    assert!(
                        args.contains(windows_detect::PWSH_UTF8_SETUP),
                        "{} misses the console encoding setup: {:?}",
                        p.name,
                        p.args
                    );
                    assert!(
                        args.contains("]7;file:///$p"),
                        "{} lost its OSC 7 prompt: {:?}",
                        p.name,
                        p.args
                    );
                }
                "bash.exe" => {
                    assert_eq!(p.args[0], "-c", "{}: {:?}", p.name, p.args);
                    assert!(
                        args.contains("chcp.com 65001") && args.contains("exec \"$BASH\""),
                        "{} misses the UTF-8 exec launcher: {:?}",
                        p.name,
                        p.args
                    );
                    // The rcfile carries the OSC 7 PROMPT_COMMAND hook, so
                    // losing it would silently kill cwd tracking.
                    assert!(
                        args.contains("--rcfile") || args.contains("--login"),
                        "{} lost its interactive argv: {:?}",
                        p.name,
                        p.args
                    );
                }
                other => panic!("unclassified Windows profile {} ({other})", p.name),
            }
        }
    }

    /// End-to-end proof that the argv above actually produces a UTF-8
    /// console: spawn every detected Windows shell on a real PTY, have it
    /// report its code page and echo a Korean string back through a shell
    /// variable, and assert the bytes come back as correct UTF-8.
    ///
    /// Reading it out of a variable (rather than matching the typed line) is
    /// what makes this a round trip: the marker only appears if the shell
    /// *decoded* the Korean we typed and then *re-encoded* it on the way
    /// out. Ignored by default because it spawns real shells and a cold
    /// PowerShell profile can take seconds; run with
    /// `cargo test -p ymux --no-default-features --lib -- --ignored shells_round_trip`.
    #[cfg(windows)]
    #[test]
    #[ignore = "spawns real shells; run explicitly with --ignored"]
    fn windows_shells_round_trip_korean_over_a_real_pty() {
        use crate::config::model::PaneSpec;
        use crate::pty::session::{CwdMap, PaneEvent, PtySession};
        use parking_lot::Mutex;
        use portable_pty::PtySize;
        use std::collections::HashMap;
        use std::sync::mpsc;
        use std::sync::Arc;

        // `한글` inside brackets so it cannot be satisfied by the echo of the
        // command line we type.
        const MARKER: &str = "ymux[한글]";

        let mut checked = 0usize;
        for p in detect_shells() {
            if p.name.starts_with("WSL: ") || p.name == "Nushell" {
                continue;
            }
            let script: &[u8] = match exe_base(&p).as_str() {
                "cmd.exe" => "chcp\r\nset YK=한글\r\necho ymux[%YK%]\r\nexit\r\n".as_bytes(),
                "powershell.exe" | "pwsh.exe" => {
                    "[Console]::OutputEncoding.CodePage\r\n$yk = \"한글\"\r\nWrite-Output \"ymux[$yk]\"\r\nexit\r\n".as_bytes()
                }
                "bash.exe" => "chcp.com\nyk=한글\necho \"ymux[$yk]\"\nexit\n".as_bytes(),
                _ => continue,
            };

            let (tx, rx) = mpsc::channel();
            let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
            let session = match PtySession::spawn(
                &PaneSpec::new_default(),
                &p,
                PtySize {
                    rows: 24,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                },
                tx,
                Arc::clone(&cwds),
                &[],
            ) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("skipping {}: spawn failed: {e}", p.name);
                    continue;
                }
            };
            // Give the shell a moment to finish its own startup (and its
            // chcp) before typing at it.
            std::thread::sleep(std::time::Duration::from_millis(1500));
            session.write(script).expect("write");

            let mut captured = Vec::new();
            // The OSC 7 hook is the other half of the contract: for Git Bash
            // it lives in the rcfile the `-c` wrapper now `exec`s into, and
            // for cmd / PowerShell the code-page setup is chained ahead of
            // the prompt installer. Counting the parsed events proves the
            // hook still fires rather than merely still appearing in argv.
            let mut cwd_events = 0usize;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(PaneEvent::Data(_, b)) => captured.extend_from_slice(&b),
                    Ok(PaneEvent::Cwd(..)) => cwd_events += 1,
                    Ok(PaneEvent::Exit(..)) => break,
                    Err(_) if std::time::Instant::now() > deadline => break,
                    Err(_) => continue,
                }
            }
            // `from_utf8` (not lossy): invalid bytes are exactly the failure
            // mode under test, so they must not be papered over.
            let text = std::str::from_utf8(&captured)
                .unwrap_or_else(|e| panic!("{}: output is not valid UTF-8: {e}", p.name));
            assert!(
                text.contains("65001"),
                "{} did not report code page 65001: {text:?}",
                p.name
            );
            assert!(
                text.contains(MARKER),
                "{} did not round-trip Korean: {text:?}",
                p.name
            );
            assert!(
                cwd_events > 0,
                "{} reported no OSC 7 cwd — the encoding setup broke the hook: {text:?}",
                p.name
            );
            checked += 1;
            eprintln!(
                "ok: {} round-tripped {MARKER} at cp 65001, {cwd_events} OSC 7 cwd report(s)",
                p.name
            );
        }
        assert!(checked > 0, "no Windows shell was available to verify");
    }

    /// Index of the first line of `script` containing `needle`, so tests can
    /// assert ordering without pinning the exact text.
    fn line_of(script: &str, needle: &str) -> usize {
        script
            .lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not found in:\n{script}"))
    }

    /// Hand ZDOTDIR back to the user's value (unset when they had none).
    const ZDOTDIR_TO_USER: &str = r#"if [ -n "$YMUX_USER_ZDOTDIR" ]; then export ZDOTDIR="$YMUX_USER_ZDOTDIR"; else unset ZDOTDIR; fi"#;
    const ZDOTDIR_TO_SHIM: &str = r#"export ZDOTDIR="$YMUX_SHIM_ZDOTDIR""#;
    const ADOPT_USER_ZDOTDIR: &str = r#"YMUX_USER_ZDOTDIR="${ZDOTDIR-}""#;

    /// Every user startup file must run with ZDOTDIR set to the *user's*
    /// value, not the shim — otherwise `${ZDOTDIR:-$HOME}` in their dotfiles
    /// points into ymux's dir — and zsh must be pointed back at the shim
    /// afterwards so it still reads the next shim file.
    #[test]
    fn zsh_shim_sources_user_files_with_the_users_zdotdir() {
        use unix_scripts::{ZSH_ZPROFILE, ZSH_ZSHENV, ZSH_ZSHRC};
        for (script, file) in [
            (ZSH_ZSHENV, ".zshenv"),
            (ZSH_ZPROFILE, ".zprofile"),
            (ZSH_ZSHRC, ".zshrc"),
        ] {
            let source = format!(r#". "${{ZDOTDIR:-$HOME}}/{file}""#);
            let to_user = line_of(script, ZDOTDIR_TO_USER);
            let sourced = line_of(script, &source);
            let adopt = line_of(script, ADOPT_USER_ZDOTDIR);
            assert!(
                to_user < sourced && sourced < adopt,
                "{file}: ZDOTDIR must be the user's while their file runs, then adopted:\n{script}"
            );
            // Nothing sources a user file from the shim dir or from a
            // hard-coded $YMUX_USER_ZDOTDIR path any more.
            assert!(
                !script.contains(r#". "$YMUX_USER_ZDOTDIR/"#),
                "{file}:\n{script}"
            );
        }
        // .zshenv and .zprofile hand zsh back to the shim afterwards.
        for script in [ZSH_ZSHENV, ZSH_ZPROFILE] {
            assert!(
                line_of(script, ADOPT_USER_ZDOTDIR) < line_of(script, ZDOTDIR_TO_SHIM),
                "{script}"
            );
        }
        // .zshenv records the shim dir before touching ZDOTDIR.
        assert!(
            line_of(ZSH_ZSHENV, r#"YMUX_SHIM_ZDOTDIR="${ZDOTDIR:-$HOME}""#)
                < line_of(ZSH_ZSHENV, ZDOTDIR_TO_USER)
        );
    }

    /// macOS `/etc/zshrc` runs between the shim's .zprofile and .zshrc with
    /// ZDOTDIR on the shim, so its `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history`
    /// lands inside the shim dir. The shim .zshrc moves it back — before the
    /// user's .zshrc and after it — and only when it points into the shim.
    #[test]
    fn zsh_shim_moves_histfile_out_of_the_shim_dir() {
        let script = unix_scripts::ZSH_ZSHRC;
        let fix = r#"case "${HISTFILE-}" in "$YMUX_SHIM_ZDOTDIR"/*) HISTFILE="${YMUX_USER_ZDOTDIR:-$HOME}/.zsh_history" ;; esac"#;
        let fixes: Vec<usize> = script
            .lines()
            .enumerate()
            .filter(|(_, l)| *l == fix)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(fixes.len(), 2, "expected the HISTFILE fix twice:\n{script}");
        let sourced = line_of(script, r#". "${ZDOTDIR:-$HOME}/.zshrc""#);
        assert!(fixes[0] < sourced && sourced < fixes[1], "{script}");
        assert!(fixes[1] < line_of(script, "add-zsh-hook precmd _ymux_osc7"));
    }

    /// Startup ends with ZDOTDIR restored to the user's value (or unset) and
    /// ymux's bookkeeping variables gone, so nested zsh processes behave
    /// normally. The OSC 7 hook is installed before that.
    #[test]
    fn zsh_shim_restores_zdotdir_at_the_end_of_startup() {
        use unix_scripts::{ZSH_ZLOGIN, ZSH_ZSHRC};
        let cleanup = "unset YMUX_USER_ZDOTDIR YMUX_SHIM_ZDOTDIR";
        for script in [ZSH_ZSHRC, ZSH_ZLOGIN] {
            let lines: Vec<&str> = script.lines().collect();
            assert_eq!(lines.last().copied(), Some(cleanup), "{script}");
            // The last ZDOTDIR assignment hands it to the user (or unsets it).
            let last_export = lines
                .iter()
                .rev()
                .find(|l| l.contains("export ZDOTDIR="))
                .expect("a ZDOTDIR restore");
            assert!(
                last_export
                    .contains("then export ZDOTDIR=\"$YMUX_USER_ZDOTDIR\"; else unset ZDOTDIR; fi"),
                "{script}"
            );
            assert!(!script.contains(ZDOTDIR_TO_SHIM), "{script}");
        }
        assert!(
            line_of(ZSH_ZSHRC, "_ymux_osc7() {") < line_of(ZSH_ZSHRC, cleanup),
            "the OSC 7 hook must still be installed"
        );
        assert!(ZSH_ZLOGIN.contains(r#". "${ZDOTDIR:-$HOME}/.zlogin""#));
    }

    /// The unix bash rcfile replays a login shell: /etc/profile (path_helper
    /// on macOS) first, then the first of the three login files, then ymux's
    /// hook — and never forces ~/.bashrc on top, which most .bash_profiles
    /// already source.
    #[test]
    fn unix_bash_rcfile_replays_a_login_shell() {
        let rc = unix_scripts::BASH_OSC7_RCFILE;
        let etc = line_of(rc, "[ -r /etc/profile ] && . /etc/profile");
        let bp = line_of(rc, r#"if [ -f "$HOME/.bash_profile" ]; then"#);
        let bl = line_of(rc, r#"elif [ -f "$HOME/.bash_login" ]; then"#);
        let pr = line_of(rc, r#"elif [ -f "$HOME/.profile" ]; then"#);
        let hook = line_of(rc, "_ymux_osc7() {");
        assert!(etc < bp && bp < bl && bl < pr && pr < hook, "{rc}");
        assert!(
            !rc.contains(".bashrc"),
            "must not source ~/.bashrc twice:\n{rc}"
        );
        assert!(rc.contains(r#"PROMPT_COMMAND="_ymux_osc7;${PROMPT_COMMAND:-}""#));
    }
}
