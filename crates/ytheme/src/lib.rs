//! Shared theme and configuration directory utilities for the ymux tool
//! family (ymux, ymon, ydir, ycode, ylauncher).
//!
//! Every y* tool reads its colors from `%APPDATA%\ymux\theme.toml` (Windows)
//! or `~/.config/ymux/theme.toml` (Unix dev hosts). If the file doesn't
//! exist, the built-in Night Owl–inspired defaults are used.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Return the root configuration directory for the ymux family.
/// `%APPDATA%\ymux` on Windows, `~/.config/ymux` on others.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ymux"))
}

/// Ensure the config directory tree exists. Returns the directory path.
pub fn ensure_config_dir() -> std::io::Result<PathBuf> {
    let dir = config_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no config directory"))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the path where tool-specific config lives.
/// e.g. `config_dir_for("ymon")` → `%APPDATA%\ymux\ymon\`
pub fn config_dir_for(tool: &str) -> Option<PathBuf> {
    config_dir().map(|d| d.join(tool))
}

/// Return the path to `theme.toml`.
pub fn theme_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("theme.toml"))
}

/// A single color expressed as an RGB hex string (`"#rrggbb"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct HexColor(pub String);

impl HexColor {
    pub fn new(hex: &str) -> Self {
        Self(hex.to_string())
    }

    /// Parse into (r, g, b). Returns None on malformed input.
    pub fn to_rgb(&self) -> Option<(u8, u8, u8)> {
        let s = self.0.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some((r, g, b))
    }
}

impl Default for HexColor {
    fn default() -> Self {
        Self("#d6deeb".to_string())
    }
}

/// Colors used by yCode's syntax highlighter. Maps to TextMate-style scope
/// selectors (keyword.*, string, comment, constant.numeric, entity.name.function,
/// entity.name.type, etc.). Defaults are Night Owl-inspired and harmonize
/// with the main palette.
///
/// Users override these by adding a `[syntax]` table to `theme.toml`:
///
/// ```toml
/// [syntax]
/// keyword = "#c792ea"
/// string  = "#ecc48d"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SyntaxColors {
    pub keyword: HexColor,
    pub string: HexColor,
    pub comment: HexColor,
    pub number: HexColor,
    pub function: HexColor,
    pub type_name: HexColor,
    pub variable: HexColor,
    pub punctuation: HexColor,
}

impl Default for SyntaxColors {
    fn default() -> Self {
        Self {
            keyword: HexColor::new("#c792ea"),     // purple
            string: HexColor::new("#ecc48d"),      // soft yellow
            comment: HexColor::new("#637777"),     // muted slate
            number: HexColor::new("#f78c6c"),      // soft orange
            function: HexColor::new("#82aaff"),    // bright blue
            type_name: HexColor::new("#ffcb8b"),   // amber
            variable: HexColor::new("#d6deeb"),    // default fg
            punctuation: HexColor::new("#7fdbca"), // accent (teal)
        }
    }
}

/// The full theme definition shared across all y* tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub bg: HexColor,
    pub bg_alt: HexColor,
    pub bg_hover: HexColor,
    pub fg: HexColor,
    pub fg_muted: HexColor,
    pub accent: HexColor,
    pub border: HexColor,
    pub status_ok: HexColor,
    pub status_warn: HexColor,
    pub status_critical: HexColor,
    pub syntax: SyntaxColors,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: HexColor::new("#0b0f14"),
            bg_alt: HexColor::new("#111820"),
            bg_hover: HexColor::new("#1a2230"),
            fg: HexColor::new("#d6deeb"),
            fg_muted: HexColor::new("#6a7a8a"),
            accent: HexColor::new("#7fdbca"),
            border: HexColor::new("#1e2a38"),
            status_ok: HexColor::new("#7fdbca"),
            status_warn: HexColor::new("#e5c07b"),
            status_critical: HexColor::new("#ef6b73"),
            syntax: SyntaxColors::default(),
        }
    }
}

impl Theme {
    /// Load from `theme.toml`. Returns the default theme if the file
    /// doesn't exist or fails to parse.
    pub fn load() -> Self {
        Self::load_from_path(theme_path().as_deref())
    }

    /// Load from an explicit path. Returns the default on any error.
    pub fn load_from_path(path: Option<&Path>) -> Self {
        let Some(p) = path else {
            return Self::default();
        };
        match std::fs::read_to_string(p) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Serialize the current theme to TOML and write it to `theme.toml`.
    pub fn save(&self) -> std::io::Result<()> {
        let dir = ensure_config_dir()?;
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join("theme.toml"), text)
    }
}

/// Convenience: load the theme (or defaults) and return it. Every y* tool
/// calls this at startup.
pub fn load_theme() -> Theme {
    Theme::load()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_has_valid_colors() {
        let theme = Theme::default();
        assert!(theme.bg.to_rgb().is_some());
        assert!(theme.fg.to_rgb().is_some());
        assert!(theme.accent.to_rgb().is_some());
        assert!(theme.status_ok.to_rgb().is_some());
        assert!(theme.status_warn.to_rgb().is_some());
        assert!(theme.status_critical.to_rgb().is_some());
    }

    #[test]
    fn hex_color_parsing() {
        let c = HexColor::new("#7fdbca");
        assert_eq!(c.to_rgb(), Some((0x7f, 0xdb, 0xca)));

        let bad = HexColor::new("invalid");
        assert_eq!(bad.to_rgb(), None);

        let short = HexColor::new("#fff");
        assert_eq!(short.to_rgb(), None);
    }

    #[test]
    fn roundtrip_toml_serialization() {
        let theme = Theme::default();
        let text = toml::to_string_pretty(&theme).unwrap();
        let parsed: Theme = toml::from_str(&text).unwrap();
        assert_eq!(parsed.bg, theme.bg);
        assert_eq!(parsed.accent, theme.accent);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let partial = r##"
            bg = "#000000"
            fg = "#ffffff"
        "##;
        let theme: Theme = toml::from_str(partial).unwrap();
        assert_eq!(theme.bg, HexColor::new("#000000"));
        assert_eq!(theme.fg, HexColor::new("#ffffff"));
        // Unspecified fields use defaults
        assert_eq!(theme.accent, Theme::default().accent);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let theme = Theme::load_from_path(Some(Path::new("/nonexistent/theme.toml")));
        assert_eq!(theme.bg, Theme::default().bg);
    }

    #[test]
    fn config_dir_is_not_empty() {
        // On any system with a home directory this should return Some
        if let Some(dir) = config_dir() {
            assert!(dir.to_string_lossy().contains("ymux"));
        }
    }

    #[test]
    fn config_dir_for_appends_tool_name() {
        if let Some(dir) = config_dir_for("ymon") {
            assert!(dir.to_string_lossy().ends_with("ymon"));
        }
    }
}
