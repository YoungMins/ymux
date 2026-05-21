//! Tauri commands backing the in-app Settings panel.
//!
//! Two responsibilities:
//!   1. Load / save the shared `ytheme::Theme` so the frontend can let users
//!      edit yCode's syntax colors (and the broader palette) without opening
//!      a text editor. The theme is written to `<config_dir>/theme.toml`,
//!      which every y* TUI tool re-reads on next launch.
//!   2. Open the theme file or the ymux config directory with the OS
//!      default app, for power users who want raw TOML editing or backup.
//!
//! Path scoping: the `open_config_path` command only opens known paths
//! (`theme.toml`, the config directory) — never arbitrary frontend input —
//! so there's no command-injection surface from the JS side.

use anyhow::Context;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPathKind {
    Theme,
    Folder,
}

#[tauri::command]
pub fn load_syntax_theme() -> Result<ytheme::Theme, String> {
    Ok(ytheme::Theme::load())
}

#[tauri::command]
pub fn save_syntax_theme(theme: ytheme::Theme) -> Result<(), String> {
    theme.save().map_err(|e| format!("save failed: {e}"))
}

#[tauri::command]
pub fn open_config_path(kind: ConfigPathKind) -> Result<(), String> {
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
