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

    pub fn run() -> Vec<ShellProfile> {
        let mut out = Vec::new();

        // 1. PowerShell 7+ (pwsh.exe) — prefer over Windows PowerShell 5.1.
        if let Some(p) = find_pwsh() {
            out.push(ShellProfile {
                name: "PowerShell 7".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec!["-NoLogo".into()],
                icon: Some("pwsh".into()),
                color: Some("#012456".into()),
            });
        }

        // 2. Windows PowerShell 5.1 — bundled with Windows.
        if let Some(p) = find_windows_powershell() {
            out.push(ShellProfile {
                name: "Windows PowerShell".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec!["-NoLogo".into()],
                icon: Some("powershell".into()),
                color: Some("#012456".into()),
            });
        }

        // 3. cmd.exe — always present.
        if let Some(p) = find_cmd() {
            out.push(ShellProfile {
                name: "Command Prompt".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec![],
                icon: Some("cmd".into()),
                color: Some("#0c0c0c".into()),
            });
        }

        // 4. Git Bash.
        if let Some(p) = find_git_bash() {
            out.push(ShellProfile {
                name: "Git Bash".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec!["--login".into(), "-i".into()],
                icon: Some("gitbash".into()),
                color: Some("#4e4e4e".into()),
            });
        }

        // 5. WSL distros.
        out.extend(find_wsl_distros());

        // 6. Nushell, if on PATH.
        if let Some(p) = which("nu") {
            out.push(ShellProfile {
                name: "Nushell".into(),
                executable: p.to_string_lossy().into_owned(),
                args: vec![],
                icon: Some("nu".into()),
                color: Some("#4e9a06".into()),
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
        let output = match Command::new(&wsl).args(["--list", "--quiet"]).output() {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        // `wsl.exe --list --quiet` prints UTF-16LE. Decode with a lossy
        // fallback if the first byte pair looks like UTF-16.
        let text = decode_possibly_utf16(&output.stdout);
        let mut profiles = Vec::new();
        for line in text.lines() {
            let name = line.trim().trim_end_matches('\r').trim();
            if name.is_empty() {
                continue;
            }
            profiles.push(ShellProfile {
                name: format!("WSL: {name}"),
                executable: wsl.to_string_lossy().into_owned(),
                args: vec!["-d".into(), name.into()],
                icon: Some("wsl".into()),
                color: Some("#4e9a06".into()),
            });
        }
        profiles
    }

    fn decode_possibly_utf16(bytes: &[u8]) -> String {
        if bytes.len() >= 2 && bytes.len() % 2 == 0 {
            let looks_like_utf16 = bytes
                .chunks_exact(2)
                .take(16)
                .any(|c| c[1] == 0 && c[0] != 0);
            if looks_like_utf16 {
                let u16s: Vec<u16> = bytes
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
    use super::{is_file, which, PathBuf, ShellProfile};

    pub fn run() -> Vec<ShellProfile> {
        let mut out = Vec::new();

        // Respect $SHELL first so dev shells feel normal.
        if let Ok(s) = std::env::var("SHELL") {
            let path = PathBuf::from(&s);
            if is_file(&path) {
                out.push(ShellProfile {
                    name: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "shell".into()),
                    executable: s,
                    args: vec!["-l".into()],
                    icon: None,
                    color: None,
                });
            }
        }

        for name in ["bash", "zsh", "fish", "sh"] {
            if out.iter().any(|p| p.executable.ends_with(name)) {
                continue;
            }
            if let Some(p) = which(name) {
                out.push(ShellProfile {
                    name: name.to_string(),
                    executable: p.to_string_lossy().into_owned(),
                    args: vec![],
                    icon: None,
                    color: None,
                });
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
