//! Install / uninstall ymux's Claude Code hooks in `~/.claude/settings.json`.
//!
//! Every hook ymux adds is a `type: "http"` handler that POSTs the event to
//! ymux's loopback receiver (`crate::hook_http`):
//!
//! ```json
//! { "type": "http", "url": "http://127.0.0.1:<port>/ymux-agent-hook", "timeout": 2,
//!   "headers": { "X-Ymux-Pane": "${YMUX_PANE_ID}", "X-Ymux-Token": "${YMUX_HOOK_TOKEN}" },
//!   "allowedEnvVars": ["YMUX_PANE_ID", "YMUX_HOOK_TOKEN"] }
//! ```
//!
//! The URL's `/ymux-agent-hook` path on a loopback host is how we recognise
//! our own entries, so install is additive and idempotent and uninstall never
//! touches anything else (CLAUDE.md rule 12). Entries from before the switch
//! to http — `"<abs y>" agent-hook claude --ymux-agent-hook` commands — carry
//! [`LEGACY_MARKER`]; install replaces them and uninstall removes them, so an
//! upgraded user is never left calling the deleted `y` binary. The merge
//! functions are pure over `serde_json::Value`, which needs the
//! `preserve_order` feature so the user's key order survives the rewrite.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::{YmuxError, YmuxResult};
use crate::hook_http::{HOOK_PATH, TOKEN_ENV};

/// Ownership marker of the retired `y agent-hook` command entries.
pub const LEGACY_MARKER: &str = "--ymux-agent-hook";

/// Env var carrying the pane id into every PTY (set in `pty::session`).
pub const PANE_ENV: &str = "YMUX_PANE_ID";

/// Pattern ymux adds to a user-level `allowedHttpHookUrls` that already
/// exists (never creates one: defining the key blocks every http hook not on
/// it). Port-independent, so a port change never needs a second entry.
pub const ALLOWED_URL_PATTERN: &str = "http://127.0.0.1:*/ymux-agent-hook";

/// Seconds Claude Code waits for the receiver. Its default for http hooks is
/// 600 s: a stalled ymux must not stall every tool call. `SessionEnd`'s
/// shared 1.5 s budget is raised to match this, which is harmless.
pub const HOOK_TIMEOUT_SECS: u64 = 2;

/// Hook events ymux subscribes to, with the tool matcher (if the event takes
/// one). No `SessionStart`: Claude Code runs only `command` and `mcp_tool`
/// handlers for it, never `http` (hooks reference, "Prompt-based hooks").
/// The lead therefore appears on the first `UserPromptSubmit` (or the
/// process scan), which also carries the session id.
pub const HOOK_EVENTS: &[(&str, Option<&str>)] = &[
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

/// `http://127.0.0.1:<port>/ymux-agent-hook`. Literal port: Claude Code
/// interpolates env vars into header values only, never into the URL.
pub fn hook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}{HOOK_PATH}")
}

/// The handler object ymux installs under every event.
pub fn hook_entry(port: u16) -> Value {
    json!({
        "type": "http",
        "url": hook_url(port),
        "timeout": HOOK_TIMEOUT_SECS,
        "headers": {
            "X-Ymux-Pane": format!("${{{PANE_ENV}}}"),
            "X-Ymux-Token": format!("${{{TOKEN_ENV}}}"),
        },
        "allowedEnvVars": [PANE_ENV, TOKEN_ENV],
    })
}

/// A retired `y agent-hook … --ymux-agent-hook` command entry.
fn is_legacy(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(LEGACY_MARKER))
}

/// An http entry pointing at ymux's receiver: loopback host, our path, any
/// port (so a port change is a refresh, not a second entry).
fn is_ours_http(entry: &Value) -> bool {
    if entry.get("type").and_then(Value::as_str) != Some("http") {
        return false;
    }
    let Some(url) = entry
        .get("url")
        .and_then(Value::as_str)
        .and_then(|u| url::Url::parse(u).ok())
    else {
        return false;
    };
    url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"))
        && url.path() == HOOK_PATH
}

fn is_ours(entry: &Value) -> bool {
    is_legacy(entry) || is_ours_http(entry)
}

/// Remove every hook entry `drop(event, entry)` selects, then any group and
/// event array that removal left empty. Groups and events it didn't touch —
/// even already-empty ones — stay. `retain` throughout, never `remove`
/// (under `preserve_order` that is `swap_remove`, which reorders). Returns
/// whether anything was removed.
fn strip_entries(hooks: &mut Map<String, Value>, drop: impl Fn(&str, &Value) -> bool) -> bool {
    let mut changed = false;
    hooks.retain(|event, groups| {
        let Some(list) = groups.as_array_mut() else {
            return true;
        };
        let mut touched = false;
        list.retain_mut(|group| {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = entries.len();
            entries.retain(|e| !drop(event, e));
            if entries.len() == before {
                return true;
            }
            touched = true;
            !entries.is_empty()
        });
        changed |= touched;
        !(touched && list.is_empty())
    });
    changed
}

/// Append each of `items` missing from the array at `root[key]` — only when
/// that key already holds an array; an absent key is never created.
fn extend_existing_list(root: &mut Map<String, Value>, key: &str, items: &[&str]) {
    if let Some(list) = root.get_mut(key).and_then(Value::as_array_mut) {
        for item in items {
            if !list.iter().any(|v| v.as_str() == Some(item)) {
                list.push(Value::String((*item).to_string()));
            }
        }
    }
}

/// Remove `items` from the array at `root[key]`, keeping the key (an
/// emptied `allowedHttpHookUrls` means what the user's empty list meant
/// before install). Returns whether anything was removed.
fn prune_list(root: &mut Map<String, Value>, key: &str, items: &[&str]) -> bool {
    let Some(list) = root.get_mut(key).and_then(Value::as_array_mut) else {
        return false;
    };
    let before = list.len();
    list.retain(|v| !v.as_str().is_some_and(|s| items.contains(&s)));
    list.len() != before
}

const ALLOW_URLS_KEY: &str = "allowedHttpHookUrls";
const ALLOW_ENV_KEY: &str = "httpHookAllowedEnvVars";
const OUR_ENV_VARS: &[&str] = &[PANE_ENV, TOKEN_ENV];

/// Point ymux's hooks at the receiver on `port`: drop every retired `y`
/// command entry (any event, `SessionStart` included), refresh each ymux
/// http entry in place, add one under every [`HOOK_EVENTS`] event that has
/// none, and — only where the user already restricts http hooks — add ymux
/// to `allowedHttpHookUrls` / `httpHookAllowedEnvVars`. Foreign keys and
/// hooks are untouched and keep their order. `Err` (settings left as they
/// were) when the file's shape is not what Claude Code documents.
pub fn install_hooks(settings: &mut Value, port: u16) -> Result<(), String> {
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
    let entry = hook_entry(port);
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")?;
    strip_entries(hooks, |event, e| {
        is_legacy(e) || (is_ours_http(e) && !HOOK_EVENTS.iter().any(|(ev, _)| *ev == event))
    });
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
            for e in entries.iter_mut() {
                if is_ours_http(e) {
                    *e = entry.clone();
                    found = true;
                }
            }
        }
        if !found {
            let mut group = Map::new();
            if let Some(m) = matcher {
                group.insert("matcher".into(), Value::String((*m).into()));
            }
            group.insert("hooks".into(), Value::Array(vec![entry.clone()]));
            groups.push(Value::Object(group));
        }
    }
    extend_existing_list(root, ALLOW_URLS_KEY, &[ALLOWED_URL_PATTERN]);
    extend_existing_list(root, ALLOW_ENV_KEY, OUR_ENV_VARS);
    Ok(())
}

/// Remove every ymux hook entry — http and retired `y` command alike — then
/// any group and event array (and finally the `hooks` object) that removal
/// left empty, and ymux's own items from the http allowlists. Returns
/// whether anything changed.
pub fn uninstall_hooks(settings: &mut Value) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let mut changed = prune_list(root, ALLOW_URLS_KEY, &[ALLOWED_URL_PATTERN]);
    changed |= prune_list(root, ALLOW_ENV_KEY, OUR_ENV_VARS);
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return changed;
    };
    let stripped = strip_entries(hooks, |_, e| is_ours(e));
    let emptied = stripped && hooks.is_empty();
    if emptied {
        root.retain(|k, _| k != "hooks");
    }
    changed || stripped
}

/// `~/.claude/settings.json`, Claude Code's user-level settings.
pub fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
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

/// The file a write to `path` should actually replace: `path` resolved
/// through any symlinks when it exists, so a symlinked `settings.json` (e.g.
/// managed by a dotfiles repo) stays a link and its target gets the new
/// content. A missing file is written at `path` itself.
fn write_target(path: &Path) -> YmuxResult<PathBuf> {
    match fs::canonicalize(path) {
        Ok(real) => Ok(real),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(e.into()),
    }
}

/// Atomically replace `target` with `text`: write a sibling temp file, flush
/// it to disk, rename it over `target`. The temp file never outlives a
/// failure.
fn replace_file(target: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = sibling(target, "ymux-tmp");
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        // Durable before the rename, or a crash could leave an empty file
        // where the user's settings were.
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Atomic write (temp file + rename) through symlinks. Before the first write
/// ever, the original is copied to `settings.json.ymux-bak`.
fn write_settings(path: &Path, value: &Value) -> YmuxResult<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = sibling(path, "ymux-bak");
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    replace_file(&write_target(path)?, &text)?;
    Ok(())
}

/// Install (or refresh) ymux's hooks in the settings file at `path`.
pub fn install_at(path: &Path, port: u16) -> YmuxResult<()> {
    let original = read_settings(path)?;
    let mut next = original
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()));
    install_hooks(&mut next, port).map_err(YmuxError::Config)?;
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

/// Env var that opts a debug build into the startup hook refresh.
pub const DEV_HOOKS_ENV: &str = "YMUX_DEV_AGENT_HOOKS";

/// May startup rewrite the hooks (and persist a newly chosen receiver port)?
/// Always in release builds. A debug build (`tauri dev`) shares the live
/// config, and when the release ymux already holds the port it listens on
/// another one — refreshing would repoint the user's real
/// `~/.claude/settings.json` at the dev build. Only when
/// `YMUX_DEV_AGENT_HOOKS=1` (`env_value`) asks for it.
pub fn startup_refresh_allowed(debug_build: bool, env_value: Option<&str>) -> bool {
    !debug_build || env_value == Some("1")
}

/// Apply the `agent_tracking` setting to `~/.claude/settings.json`, pointing
/// the hooks at the receiver on `port`.
pub fn set_enabled(enabled: bool, port: u16) -> YmuxResult<()> {
    let path = claude_settings_path()
        .ok_or_else(|| YmuxError::Other("cannot resolve the home directory".into()))?;
    if enabled {
        install_at(&path, port)
    } else {
        uninstall_at(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A user's settings with foreign hooks (a foreign http hook on loopback
    /// among them) and http allowlists, keys deliberately out of alphabetical
    /// order so an order-destroying rewrite is caught.
    const FOREIGN: &str = r#"{
  "model": "opus",
  "allowedHttpHookUrls": ["https://audit.example/*"],
  "hooks": {
    "Notification": [{ "hooks": [{ "type": "command", "command": "notify-send hi" }] }],
    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "guard.sh" }] }],
    "PostToolUse": [{ "hooks": [{ "type": "http", "url": "http://127.0.0.1:9999/audit" }] }]
  },
  "httpHookAllowedEnvVars": ["AUDIT_TOKEN"],
  "permissions": { "allow": [] }
}"#;

    const PORT: u16 = 41234;

    fn foreign() -> Value {
        serde_json::from_str(FOREIGN).expect("fixture")
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object().expect("object").keys().cloned().collect()
    }

    /// Every ymux-owned entry (either shape) under `event`.
    fn ours_in(v: &Value, event: &str) -> Vec<Value> {
        v["hooks"][event]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                    .filter(is_ours)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A retired `y` entry, as v0.11 and earlier installed it.
    fn legacy_entry() -> Value {
        json!({ "type": "command",
                "command": "\"C:/Program Files/ymux/y.exe\" agent-hook claude --ymux-agent-hook",
                "timeout": 5 })
    }

    #[test]
    fn hook_entry_is_the_documented_http_shape() {
        assert_eq!(
            hook_entry(PORT),
            json!({
                "type": "http",
                "url": "http://127.0.0.1:41234/ymux-agent-hook",
                "timeout": 2,
                "headers": {
                    "X-Ymux-Pane": "${YMUX_PANE_ID}",
                    "X-Ymux-Token": "${YMUX_HOOK_TOKEN}"
                },
                "allowedEnvVars": ["YMUX_PANE_ID", "YMUX_HOOK_TOKEN"]
            })
        );
        assert!(is_ours(&hook_entry(PORT)));
    }

    #[test]
    fn ownership_needs_a_loopback_host_and_our_path() {
        assert!(is_ours_http(
            &json!({ "type": "http", "url": "http://localhost:1/ymux-agent-hook" })
        ));
        for foreign in [
            json!({ "type": "http", "url": "http://127.0.0.1:9999/audit" }),
            json!({ "type": "http", "url": "https://evil.example/ymux-agent-hook" }),
            json!({ "type": "http", "url": "http://127.0.0.1:1/ymux-agent-hook/x" }),
            json!({ "type": "http" }),
            json!({ "type": "command", "command": "curl http://127.0.0.1:1/ymux-agent-hook" }),
        ] {
            assert!(!is_ours(&foreign), "{foreign}");
        }
        assert!(is_ours(&legacy_entry()));
    }

    #[test]
    fn events_are_all_http_capable() {
        // Claude Code never runs an http handler on SessionStart or Setup.
        for (event, _) in HOOK_EVENTS {
            assert!(!["SessionStart", "Setup"].contains(event), "{event}");
        }
    }

    #[test]
    fn install_into_empty_settings_adds_every_event() {
        let mut v = json!({});
        install_hooks(&mut v, PORT).unwrap();
        assert_eq!(keys(&v), vec!["hooks"], "no allowlist key is created");
        assert_eq!(v["hooks"].as_object().unwrap().len(), HOOK_EVENTS.len());
        for (event, matcher) in HOOK_EVENTS {
            let groups = v["hooks"][*event].as_array().unwrap();
            assert_eq!(groups.len(), 1, "{event}");
            assert_eq!(
                groups[0].get("matcher").and_then(Value::as_str),
                *matcher,
                "{event}"
            );
            assert_eq!(ours_in(&v, event), vec![hook_entry(PORT)], "{event}");
        }
    }

    #[test]
    fn install_preserves_foreign_hooks_and_key_order() {
        let mut v = foreign();
        install_hooks(&mut v, PORT).unwrap();
        assert_eq!(
            keys(&v),
            vec![
                "model",
                "allowedHttpHookUrls",
                "hooks",
                "httpHookAllowedEnvVars",
                "permissions"
            ]
        );
        let hook_keys = keys(&v["hooks"]);
        assert_eq!(
            &hook_keys[..3],
            &[
                "Notification".to_string(),
                "PreToolUse".to_string(),
                "PostToolUse".to_string()
            ]
        );
        assert_eq!(
            v["hooks"]["Notification"],
            foreign()["hooks"]["Notification"]
        );
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre[0], foreign()["hooks"]["PreToolUse"][0]);
        assert_eq!(pre[1]["matcher"], "*");
        let post = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post[0], foreign()["hooks"]["PostToolUse"][0]);
        assert_eq!(ours_in(&v, "PostToolUse"), vec![hook_entry(PORT)]);
    }

    #[test]
    fn install_extends_only_allowlists_the_user_already_has() {
        let mut v = foreign();
        install_hooks(&mut v, PORT).unwrap();
        assert_eq!(
            v["allowedHttpHookUrls"],
            json!([
                "https://audit.example/*",
                "http://127.0.0.1:*/ymux-agent-hook"
            ])
        );
        assert_eq!(
            v["httpHookAllowedEnvVars"],
            json!(["AUDIT_TOKEN", "YMUX_PANE_ID", "YMUX_HOOK_TOKEN"])
        );
        // An empty list (= "block every http hook") gets ymux added, and
        // uninstall puts it back to empty, not absent.
        let mut empty = json!({ "allowedHttpHookUrls": [] });
        install_hooks(&mut empty, PORT).unwrap();
        assert_eq!(empty["allowedHttpHookUrls"], json!([ALLOWED_URL_PATTERN]));
        assert!(uninstall_hooks(&mut empty));
        assert_eq!(empty, json!({ "allowedHttpHookUrls": [] }));
    }

    #[test]
    fn install_is_idempotent() {
        let mut once = foreign();
        install_hooks(&mut once, PORT).unwrap();
        let mut twice = once.clone();
        install_hooks(&mut twice, PORT).unwrap();
        assert_eq!(
            serde_json::to_string(&once).unwrap(),
            serde_json::to_string(&twice).unwrap()
        );
    }

    #[test]
    fn install_refreshes_the_port_in_place() {
        let mut v = foreign();
        install_hooks(&mut v, 1111).unwrap();
        install_hooks(&mut v, PORT).unwrap();
        for (event, _) in HOOK_EVENTS {
            assert_eq!(ours_in(&v, event), vec![hook_entry(PORT)], "{event}");
        }
        let mut direct = foreign();
        install_hooks(&mut direct, PORT).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&direct).unwrap(),
            "a port change is a refresh, never a second entry"
        );
    }

    /// An upgraded user's settings: v0.11's `y` command entries, one of them
    /// sharing a group with a foreign hook, and one under `SessionStart`
    /// (which http hooks don't support, so nothing replaces it there).
    fn with_legacy() -> Value {
        let mut v = foreign();
        let hooks = v["hooks"].as_object_mut().unwrap();
        hooks.insert(
            "SessionStart".into(),
            json!([{ "hooks": [legacy_entry()] }]),
        );
        hooks.insert(
            "Stop".into(),
            json!([{ "hooks": [{ "type": "command", "command": "mine.sh" }, legacy_entry()] }]),
        );
        hooks["PreToolUse"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "matcher": "*", "hooks": [legacy_entry()] }));
        v
    }

    #[test]
    fn install_migrates_legacy_y_entries() {
        let mut v = with_legacy();
        install_hooks(&mut v, PORT).unwrap();
        assert!(
            v["hooks"].get("SessionStart").is_none(),
            "legacy-only event removed"
        );
        for (event, _) in HOOK_EVENTS {
            assert_eq!(ours_in(&v, event), vec![hook_entry(PORT)], "{event}");
        }
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(
            stop[0],
            json!({ "hooks": [{ "type": "command", "command": "mine.sh" }] }),
            "foreign entry sharing the legacy group survives"
        );
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2, "legacy-only group replaced, not kept empty");
        assert_eq!(pre[0], foreign()["hooks"]["PreToolUse"][0]);
        // Same result as installing over settings that never had `y`, apart
        // from the foreign `mine.sh` group.
        let serialized = serde_json::to_string(&v).unwrap();
        assert!(!serialized.contains(LEGACY_MARKER));
    }

    #[test]
    fn install_drops_our_http_entry_from_events_no_longer_subscribed() {
        let mut v = json!({ "hooks": { "SessionStart": [{ "hooks": [hook_entry(PORT)] }] } });
        install_hooks(&mut v, PORT).unwrap();
        assert!(v["hooks"].get("SessionStart").is_none());
    }

    #[test]
    fn uninstall_restores_foreign_settings_exactly() {
        let mut v = foreign();
        install_hooks(&mut v, PORT).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn uninstall_removes_both_shapes() {
        let mut v = with_legacy();
        install_hooks(&mut v, PORT).unwrap();
        // A stale http entry and a legacy one side by side, as a crashed
        // half-migration could leave them.
        v["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [legacy_entry(), hook_entry(1111)] }));
        assert!(uninstall_hooks(&mut v));
        let mut expected = foreign();
        expected["hooks"].as_object_mut().unwrap().insert(
            "Stop".into(),
            json!([{ "hooks": [{ "type": "command", "command": "mine.sh" }] }]),
        );
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&expected).unwrap()
        );
    }

    #[test]
    fn uninstall_of_legacy_only_settings_restores_them() {
        let mut v = with_legacy();
        assert!(uninstall_hooks(&mut v));
        let mut expected = foreign();
        expected["hooks"].as_object_mut().unwrap().insert(
            "Stop".into(),
            json!([{ "hooks": [{ "type": "command", "command": "mine.sh" }] }]),
        );
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&expected).unwrap()
        );
    }

    #[test]
    fn uninstall_keeps_foreign_entries_sharing_a_group() {
        let mut v = json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "mine.sh" },
            hook_entry(PORT)
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
        install_hooks(&mut v, PORT).unwrap();
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
        assert!(install_hooks(&mut json!([]), PORT).is_err());
        assert!(install_hooks(&mut json!({ "hooks": 3 }), PORT).is_err());
        let mut bad =
            json!({ "hooks": { "Stop": {}, "SessionStart": [{ "hooks": [legacy_entry()] }] } });
        let before = bad.clone();
        assert!(install_hooks(&mut bad, PORT).is_err());
        assert_eq!(bad, before, "an Err leaves the value untouched");
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
        install_at(&path, PORT).unwrap();
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
        install_at(&path, PORT).unwrap();
        let bak = sibling(&path, "ymux-bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FOREIGN);
        uninstall_at(&path).unwrap();
        install_at(&path, 1111).unwrap();
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
        install_at(&path, PORT).unwrap();
        assert_eq!(ours_in(&read(&path), "Stop"), vec![hook_entry(PORT)]);
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
        assert!(install_at(&path, PORT).is_err());
        assert!(uninstall_at(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!sibling(&path, "ymux-bak").exists());
    }

    #[test]
    fn write_target_is_the_path_itself_when_missing_and_canonical_otherwise() {
        let dir = tempdir();
        let missing = dir.join("settings.json");
        assert_eq!(write_target(&missing).unwrap(), missing);
        std::fs::write(&missing, "{}").unwrap();
        assert_eq!(
            write_target(&missing).unwrap(),
            std::fs::canonicalize(&missing).unwrap()
        );
    }

    #[test]
    fn a_failed_replace_removes_its_temp_file() {
        // A directory can't be replaced by a file on any platform, so the
        // rename fails after the temp file was written.
        let target = tempdir().join("settings.json");
        std::fs::create_dir_all(&target).unwrap();
        assert!(replace_file(&target, "{}\n").is_err());
        assert!(
            !sibling(&target, "ymux-tmp").exists(),
            "temp file left behind"
        );
        assert!(target.is_dir(), "target untouched");
    }

    #[test]
    fn replace_file_writes_the_content() {
        let target = tempdir().join("settings.json");
        std::fs::write(&target, "old").unwrap();
        replace_file(&target, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
        assert!(!sibling(&target, "ymux-tmp").exists());
    }

    /// A dotfiles-managed `settings.json` symlink must stay a symlink; the
    /// file it points at is what gets rewritten. (Unix only: creating a
    /// symlink on Windows needs Developer Mode or admin.)
    #[cfg(unix)]
    #[test]
    fn install_through_a_symlink_rewrites_the_target_and_keeps_the_link() {
        let dir = tempdir();
        let real = dir.join("dotfiles-settings.json");
        std::fs::write(&real, FOREIGN).unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        install_at(&link, PORT).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(ours_in(&read(&real), "Stop"), vec![hook_entry(PORT)]);
    }

    #[test]
    fn startup_refresh_runs_in_release_and_only_opt_in_in_debug() {
        assert!(startup_refresh_allowed(false, None));
        assert!(startup_refresh_allowed(false, Some("0")));
        assert!(!startup_refresh_allowed(true, None));
        assert!(!startup_refresh_allowed(true, Some("0")));
        assert!(!startup_refresh_allowed(true, Some("")));
        assert!(startup_refresh_allowed(true, Some("1")));
    }

    #[test]
    fn uninstall_of_a_missing_file_is_a_noop() {
        let path = tempdir().join("settings.json");
        uninstall_at(&path).unwrap();
        assert!(!path.exists());
    }
}
