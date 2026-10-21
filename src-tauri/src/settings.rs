//! Tauri commands backing the in-app Settings panel.
//!
//! Two responsibilities:
//!   1. Load / save the shared `ytheme::Theme` so the frontend can let users
//!      edit the editor pane's syntax colors (and the broader palette)
//!      without opening a text editor. The theme is written to
//!      `<config_dir>/theme.toml`.
//!   2. Open the theme file or the ymux config directory with the OS
//!      default app, for power users who want raw TOML editing or backup.
//!
//! Path scoping: the `open_config_path` command only opens known paths
//! (`theme.toml`, the config directory) — never arbitrary frontend input —
//! so there's no command-injection surface from the JS side.

use anyhow::Context;
use tauri::ipc::Request;
use tauri::Webview;

use crate::fspath::guard_local;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPathKind {
    Theme,
    Folder,
}

#[tauri::command]
pub fn load_syntax_theme(webview: Webview, request: Request<'_>) -> Result<ytheme::Theme, String> {
    guard_local(&webview, &request, "load_syntax_theme").map_err(|e| e.to_string())?;
    Ok(ytheme::Theme::load())
}

#[tauri::command]
pub fn save_syntax_theme(
    webview: Webview,
    request: Request<'_>,
    theme: ytheme::Theme,
) -> Result<(), String> {
    guard_local(&webview, &request, "save_syntax_theme").map_err(|e| e.to_string())?;
    theme.save().map_err(|e| format!("save failed: {e}"))
}

#[tauri::command]
pub fn open_config_path(
    webview: Webview,
    request: Request<'_>,
    kind: ConfigPathKind,
) -> Result<(), String> {
    guard_local(&webview, &request, "open_config_path").map_err(|e| e.to_string())?;
    let path = match kind {
        ConfigPathKind::Theme => {
            // Create the file with current defaults if it's never been saved,
            // so the user has something to look at when the editor opens.
            if let Some(p) = ytheme::theme_path() {
                if !p.exists() {
                    ytheme::Theme::default()
                        .save()
                        .map_err(|e| format!("seed theme.toml failed: {e}"))?;
                }
            }
            ytheme::theme_path()
        }
        ConfigPathKind::Folder => {
            ytheme::ensure_config_dir().map_err(|e| format!("create config dir failed: {e}"))?;
            ytheme::config_dir()
        }
    }
    .context("config directory unavailable on this platform")
    .map_err(|e| e.to_string())?;

    opener::open(&path).map_err(|e| format!("opener failed for {}: {e}", path.display()))?;
    Ok(())
}

/// The two login folders of a ymux-managed account.
#[derive(Debug, serde::Serialize)]
pub struct ManagedAccountDirs {
    pub claude_config_dir: String,
    pub codex_home: String,
}

/// Pick (and create) the login folders for a new account, so adding one
/// never asks the user for a path: `<config_dir>/accounts/<id>/{claude,codex}`.
/// Claude Code / Codex find no credentials there and ask the user to log in
/// on first start, which is how a new account gets its login.
#[tauri::command]
pub fn prepare_account_dirs(
    webview: Webview,
    request: Request<'_>,
    id: String,
) -> Result<ManagedAccountDirs, String> {
    guard_local(&webview, &request, "prepare_account_dirs").map_err(|e| e.to_string())?;
    let base = ytheme::ensure_config_dir().map_err(|e| format!("create config dir failed: {e}"))?;
    let (claude, codex) = crate::accounts::managed_dirs(&base, &id)
        .ok_or_else(|| "invalid account id".to_string())?;
    for dir in [&claude, &codex] {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("create {} failed: {e}", dir.display()))?;
    }
    Ok(ManagedAccountDirs {
        claude_config_dir: claude.display().to_string(),
        codex_home: codex.display().to_string(),
    })
}

/// Delete a managed account's login folders (`<config_dir>/accounts/<id>`)
/// after the user removed the account. The frontend deletes the profile and
/// saves first; an id still in the saved account list is refused, so a
/// stale or forged call cannot wipe a live account's logins.
///
/// Holds `commands::HOOK_WRITES`, the lock every hook-file write takes: the
/// save that dropped the profile uninstalls the hooks from this folder on a
/// worker thread (`HookTrigger::AccountsChanged`), and that rewrite must
/// not interleave with the delete (its `create_dir_all` would resurrect part
/// of the tree). Whichever runs second sees a consistent state: the
/// uninstall finds no file, or the delete removes an unhooked folder.
///
/// On macOS it also deletes, best-effort and in the background, the
/// keychain item Claude Code kept for that folder. `async`: the folder can
/// hold large transcript histories.
#[tauri::command(async)]
pub fn remove_account_dirs(
    webview: Webview,
    request: Request<'_>,
    state: tauri::State<'_, crate::commands::AppState>,
    id: String,
) -> Result<(), String> {
    guard_local(&webview, &request, "remove_account_dirs").map_err(|e| e.to_string())?;
    let base = ytheme::config_dir().ok_or_else(|| "config directory unavailable".to_string())?;
    if crate::accounts::managed_root(&base, &id).is_none() {
        return Err("invalid account id".into());
    }
    if state.config.snapshot().accounts.iter().any(|a| a.id == id) {
        return Err("account is still configured; remove it and save first".into());
    }
    // Before the folder goes: what Claude was told its config dir is.
    #[cfg(target_os = "macos")]
    let claude_env = crate::accounts::managed_claude_env(&base, &id, dirs::home_dir().as_deref());
    {
        let _hooks = crate::commands::HOOK_WRITES.lock();
        crate::accounts::remove_managed_root(&base, &id)
            .map_err(|e| format!("remove account folder failed: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = claude_env {
        let service = crate::accounts::claude_keychain_service(Some(&dir));
        std::thread::spawn(move || {
            let _ = std::process::Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", &service])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
    Ok(())
}
