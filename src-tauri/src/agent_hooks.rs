//! Install / uninstall ymux's Claude Code hooks in `~/.claude/settings.json`.
//!
//! Every hook ymux adds runs `"<abs y>" agent-hook claude --ymux-agent-hook`.
//! The trailing marker is how we recognise our own entries, so install is
//! additive and idempotent and uninstall never touches anything else. The
//! merge functions are pure over `serde_json::Value`, which needs the
//! `preserve_order` feature so the user's key order survives the rewrite.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::{YmuxError, YmuxResult};

/// Ownership marker carried by every hook command ymux installs.
pub const MARKER: &str = "--ymux-agent-hook";

/// Hook events ymux subscribes to, with the tool matcher (if the event takes one).
pub const HOOK_EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PostToolUse", Some("*")),
    ("PermissionRequest", Some("*")),
    ("Stop", None),
    ("StopFailure", None),
    ("SubagentStart", None),
    ("SubagentStop", None),
    ("PostCompact", None),
    ("SessionEnd", None),
];

/// The hook command for the `y` sidecar at `y_path`. Forward slashes so the
/// same string works whether Claude Code runs it through cmd or Git Bash.
pub fn hook_command(y_path: &Path) -> String {
    let path = y_path.to_string_lossy().replace('\\', "/");
    format!("\"{path}\" agent-hook claude {MARKER}")
}

fn is_ours(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(MARKER))
}

/// Add ymux's hook to every event in [`HOOK_EVENTS`], or refresh the command
/// of an existing ymux entry. Foreign keys and hooks are untouched and keep
/// their order. `Err` (settings left as they were) when the file's shape is
/// not what Claude Code documents.
pub fn install_hooks(settings: &mut Value, command: &str) -> Result<(), String> {
    let root = settings
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    // Validate every shape before the first mutation, so an Err leaves the
    // value exactly as it came in.
    if let Some(hooks) = root.get("hooks") {
        let hooks = hooks.as_object().ok_or("\"hooks\" is not a JSON object")?;
        for (event, _) in HOOK_EVENTS {
            if hooks.get(*event).is_some_and(|v| !v.is_array()) {
                return Err(format!("\"hooks.{event}\" is not an array"));
            }
        }
    }
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")?;
    for (event, matcher) in HOOK_EVENTS {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("\"hooks.{event}\" is not an array"))?;
        let mut found = false;
        for group in groups.iter_mut() {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for entry in entries.iter_mut() {
                if is_ours(entry) {
                    entry["command"] = Value::String(command.to_string());
                    found = true;
                }
            }
        }
        if !found {
            let mut group = Map::new();
            if let Some(m) = matcher {
                group.insert("matcher".into(), Value::String((*m).into()));
            }
            group.insert(
                "hooks".into(),
                json!([{ "type": "command", "command": command, "timeout": 5 }]),
            );
            groups.push(Value::Object(group));
        }
    }
    Ok(())
}

/// Remove every hook entry carrying [`MARKER`], then any group and event
/// array (and finally the `hooks` object) that removal left empty. Returns
/// whether anything changed.
pub fn uninstall_hooks(settings: &mut Value) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    // `retain`, never `remove`: under `preserve_order` `remove` is
    // `swap_remove` and would reorder the user's events.
    hooks.retain(|_event, groups| {
        let Some(list) = groups.as_array_mut() else {
            return true;
        };
        let mut touched = false;
        list.retain_mut(|group| {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = entries.len();
            entries.retain(|e| !is_ours(e));
            if entries.len() == before {
                return true;
            }
            touched = true;
            !entries.is_empty()
        });
        changed |= touched;
        !(touched && list.is_empty())
    });
    let emptied = changed && hooks.is_empty();
    if emptied {
        root.retain(|k, _| k != "hooks");
    }
    changed
}

/// `~/.claude/settings.json`, Claude Code's user-level settings.
pub fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// The `y` sidecar next to the running ymux executable (MSI install dir,
/// `.app/Contents/MacOS`, or `target/<profile>` under `tauri dev`).
pub fn y_sidecar_path() -> YmuxResult<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| YmuxError::Other("ymux executable has no parent directory".into()))?;
    let path = dir.join(if cfg!(windows) { "y.exe" } else { "y" });
    if path.is_file() {
        Ok(path)
    } else {
        Err(YmuxError::Other(format!(
            "y sidecar not found at {}",
            path.display()
        )))
    }
}

/// `settings.json` → `settings.json.<suffix>`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(suffix);
    PathBuf::from(s)
}

/// `Ok(None)` when the file doesn't exist; an empty file reads as `{}`.
fn read_settings(path: &Path) -> YmuxResult<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(Value::Object(Map::new()))),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| YmuxError::Config(format!("{} is not valid JSON: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Atomic write (temp file + rename). Before the first write ever, the
/// original is copied to `settings.json.ymux-bak`.
fn write_settings(path: &Path, value: &Value) -> YmuxResult<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = sibling(path, "ymux-bak");
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup)?;
    }
    let tmp = sibling(path, "ymux-tmp");
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Install (or refresh) ymux's hooks in the settings file at `path`.
pub fn install_at(path: &Path, command: &str) -> YmuxResult<()> {
    let original = read_settings(path)?;
    let mut next = original
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()));
    install_hooks(&mut next, command).map_err(YmuxError::Config)?;
    if original.as_ref() != Some(&next) {
        write_settings(path, &next)?;
    }
    Ok(())
}

/// Remove ymux's hooks from the settings file at `path`. A missing file is fine.
pub fn uninstall_at(path: &Path) -> YmuxResult<()> {
    let Some(mut value) = read_settings(path)? else {
        return Ok(());
    };
    if uninstall_hooks(&mut value) {
        write_settings(path, &value)?;
    }
    Ok(())
}

/// Apply the `agent_tracking` setting to `~/.claude/settings.json`.
pub fn set_enabled(enabled: bool) -> YmuxResult<()> {
    let path = claude_settings_path()
        .ok_or_else(|| YmuxError::Other("cannot resolve the home directory".into()))?;
    if enabled {
        install_at(&path, &hook_command(&y_sidecar_path()?))
    } else {
        uninstall_at(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A user's settings with foreign hooks, keys deliberately out of
    /// alphabetical order so an order-destroying rewrite is caught.
    const FOREIGN: &str = r#"{
  "model": "opus",
  "hooks": {
    "Notification": [{ "hooks": [{ "type": "command", "command": "notify-send hi" }] }],
    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "guard.sh" }] }]
  },
  "permissions": { "allow": [] }
}"#;

    fn foreign() -> Value {
        serde_json::from_str(FOREIGN).expect("fixture")
    }

    fn cmd() -> String {
        hook_command(Path::new("/opt/ymux/y"))
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object().expect("object").keys().cloned().collect()
    }

    fn ours_in(v: &Value, event: &str) -> Vec<String> {
        v["hooks"][event]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                    .filter(is_ours)
                    .map(|e| e["command"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn hook_command_is_quoted_forward_slashed_and_marked() {
        assert_eq!(
            hook_command(Path::new("C:\\Program Files\\ymux\\y.exe")),
            "\"C:/Program Files/ymux/y.exe\" agent-hook claude --ymux-agent-hook"
        );
    }

    #[test]
    fn install_into_empty_settings_adds_every_event() {
        let mut v = json!({});
        install_hooks(&mut v, &cmd()).unwrap();
        assert_eq!(v["hooks"].as_object().unwrap().len(), HOOK_EVENTS.len());
        for (event, matcher) in HOOK_EVENTS {
            let groups = v["hooks"][*event].as_array().unwrap();
            assert_eq!(groups.len(), 1, "{event}");
            assert_eq!(
                groups[0].get("matcher").and_then(Value::as_str),
                *matcher,
                "{event}"
            );
            assert_eq!(ours_in(&v, event), vec![cmd()], "{event}");
        }
    }

    #[test]
    fn install_preserves_foreign_hooks_and_key_order() {
        let mut v = foreign();
        install_hooks(&mut v, &cmd()).unwrap();
        assert_eq!(keys(&v), vec!["model", "hooks", "permissions"]);
        let hook_keys = keys(&v["hooks"]);
        assert_eq!(
            &hook_keys[..2],
            &["Notification".to_string(), "PreToolUse".to_string()]
        );
        assert_eq!(
            v["hooks"]["Notification"],
            foreign()["hooks"]["Notification"]
        );
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre[0], foreign()["hooks"]["PreToolUse"][0]);
        assert_eq!(pre[1]["matcher"], "*");
    }

    #[test]
    fn install_is_idempotent() {
        let mut once = foreign();
        install_hooks(&mut once, &cmd()).unwrap();
        let mut twice = once.clone();
        install_hooks(&mut twice, &cmd()).unwrap();
        assert_eq!(
            serde_json::to_string(&once).unwrap(),
            serde_json::to_string(&twice).unwrap()
        );
    }

    #[test]
    fn install_refreshes_the_y_path() {
        let mut v = json!({});
        install_hooks(&mut v, &hook_command(Path::new("/old/y"))).unwrap();
        install_hooks(&mut v, &cmd()).unwrap();
        for (event, _) in HOOK_EVENTS {
            assert_eq!(ours_in(&v, event), vec![cmd()], "{event}");
        }
    }

    #[test]
    fn uninstall_restores_foreign_settings_exactly() {
        let mut v = foreign();
        install_hooks(&mut v, &cmd()).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn uninstall_keeps_foreign_entries_sharing_a_group() {
        let mut v = json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "mine.sh" },
            { "type": "command", "command": cmd() }
        ] }] } });
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            v,
            json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "mine.sh" }] }] } })
        );
    }

    #[test]
    fn uninstall_drops_the_hooks_key_it_emptied() {
        let mut v = json!({});
        install_hooks(&mut v, &cmd()).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(v, json!({}));
    }

    #[test]
    fn uninstall_without_our_hooks_changes_nothing() {
        let mut v = foreign();
        assert!(!uninstall_hooks(&mut v));
        assert_eq!(v, foreign());
    }

    #[test]
    fn install_rejects_unexpected_shapes() {
        assert!(install_hooks(&mut json!([]), &cmd()).is_err());
        assert!(install_hooks(&mut json!({ "hooks": 3 }), &cmd()).is_err());
        assert!(install_hooks(&mut json!({ "hooks": { "Stop": {} } }), &cmd()).is_err());
    }

    /// Fresh isolated dir per test (mirrors `scrollback::tests::tempdir`).
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-agent-hooks-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
    }

    #[test]
    fn install_at_creates_a_missing_file_with_only_hooks() {
        let path = tempdir().join(".claude").join("settings.json");
        install_at(&path, &cmd()).unwrap();
        assert_eq!(keys(&read(&path)), vec!["hooks"]);
        assert!(!sibling(&path, "ymux-bak").exists(), "nothing to back up");
        assert!(
            !sibling(&path, "ymux-tmp").exists(),
            "temp file renamed away"
        );
    }

    #[test]
    fn first_write_backs_up_the_original_once() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        install_at(&path, &cmd()).unwrap();
        let bak = sibling(&path, "ymux-bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FOREIGN);
        uninstall_at(&path).unwrap();
        install_at(&path, &hook_command(Path::new("/new/y"))).unwrap();
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            FOREIGN,
            "backup never overwritten"
        );
    }

    #[test]
    fn install_then_uninstall_round_trips_the_file_content() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        install_at(&path, &cmd()).unwrap();
        assert_eq!(ours_in(&read(&path), "Stop"), vec![cmd()]);
        uninstall_at(&path).unwrap();
        assert_eq!(
            serde_json::to_string(&read(&path)).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn unparseable_settings_are_left_untouched() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(install_at(&path, &cmd()).is_err());
        assert!(uninstall_at(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!sibling(&path, "ymux-bak").exists());
    }

    #[test]
    fn uninstall_of_a_missing_file_is_a_noop() {
        let path = tempdir().join("settings.json");
        uninstall_at(&path).unwrap();
        assert!(!path.exists());
    }
}
