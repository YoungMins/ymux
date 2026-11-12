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
    // The opener hands the path to Explorer / the default editor, which run
    // outside a Store (MSIX) package: give them the real location (rule 18).
    let path = match kind {
        ConfigPathKind::Theme => crate::fspath::real_path(path),
        ConfigPathKind::Folder => crate::fspath::real_dir(&path),
    };

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
/// never asks the user for a path: `~/.ymux/accounts/<id>/{claude,codex}`.
/// In the user profile rather than the config dir, because under the
/// Microsoft Store (MSIX) package ymux's own `%APPDATA%` writes are
/// virtualised and the agent CLIs would be handed a folder they cannot see
/// (CLAUDE.md rule 18). Accounts made by older versions keep the
/// `<config_dir>/accounts/<id>` path their profile saved.
/// Claude Code / Codex find no credentials there and ask the user to log in
/// on first start, which is how a new account gets its login.
#[tauri::command]
pub fn prepare_account_dirs(
    webview: Webview,
    request: Request<'_>,
    id: String,
) -> Result<ManagedAccountDirs, String> {
    guard_local(&webview, &request, "prepare_account_dirs").map_err(|e| e.to_string())?;
    let base = crate::accounts::managed_base(dirs::home_dir().as_deref())
        .ok_or_else(|| "home directory unavailable".to_string())?;
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

/// Move a managed account's login folders to the OS trash after the user
/// removed the account: `~/.ymux/accounts/<id>`, or `<config_dir>/accounts/<id>`
/// for an account made by an older version, and nothing else
/// (`accounts::managed_bases`). The trash, not a delete: the folder holds
/// Claude transcripts and credentials. The frontend deletes the profile and
/// saves first; an id still in the saved account list is refused, so a
/// stale or forged call cannot wipe a live account's logins, and so is an
/// account a live pane was started with (its CLI is still writing there).
///
/// Holds `commands::HOOK_WRITES`, the lock every hook-file write takes: the
/// save that dropped the profile uninstalls the hooks from this folder on a
/// worker thread (`HookTrigger::AccountsChanged`), and that rewrite must
/// not interleave with the move (its `create_dir_all` would resurrect part
/// of the tree). Whichever runs second sees a consistent state: the
/// uninstall finds no file, or the move takes an unhooked folder.
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
    let home = dirs::home_dir();
    let bases = crate::accounts::managed_bases(home.as_deref(), ytheme::config_dir().as_deref());
    if bases.is_empty() || crate::accounts::managed_root(&bases[0], &id).is_none() {
        return Err("invalid account id".into());
    }
    if state.config.snapshot().accounts.iter().any(|a| a.id == id) {
        return Err("account is still configured; remove it and save first".into());
    }
    let roots = crate::accounts::existing_managed_roots(&bases, &id)
        .map_err(|e| format!("remove account folder failed: {e}"))?;
    let live = state.pty.agent_dirs_snapshot();
    if roots
        .iter()
        .any(|root| crate::accounts::root_in_use(root, live.values()))
    {
        return Err("a terminal is still using this account; close it and try again".into());
    }
    // Every root is attempted; one failing does not strand the others.
    let mut moved: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    #[cfg(target_os = "macos")]
    let mut services: Vec<String> = Vec::new();
    {
        let _hooks = crate::commands::HOOK_WRITES.lock();
        for root in &roots {
            // Before the folder goes: what Claude was told its config dir is.
            #[cfg(target_os = "macos")]
            let claude_env = crate::accounts::injected_value(
                &root.join("claude").display().to_string(),
                home.as_deref(),
            );
            match trash::delete(root) {
                Ok(()) => {
                    moved.push(root.display().to_string());
                    #[cfg(target_os = "macos")]
                    if let Some(dir) = claude_env {
                        services.push(crate::accounts::claude_keychain_service(Some(&dir)));
                    }
                }
                Err(e) => failed.push(format!("{}: {e}", root.display())),
            }
        }
    }
    #[cfg(target_os = "macos")]
    if !services.is_empty() {
        std::thread::spawn(move || {
            for service in services {
                let _ = std::process::Command::new("/usr/bin/security")
                    .args(["delete-generic-password", "-s", &service])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
    }
    if !failed.is_empty() {
        return Err(format!(
            "move account folder to trash failed for {}; moved: {}",
            failed.join("; "),
            if moved.is_empty() {
                "none".to_string()
            } else {
                moved.join(", ")
            }
        ));
    }
    Ok(())
}
