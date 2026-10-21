//! Install / uninstall ymux's Claude Code hooks in `<config dir>/settings.json`
//! for every account's Claude config dir (`~/.claude` by default).
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

use crate::config::model::AccountProfile;
use crate::error::{YmuxError, YmuxResult};
use crate::hook_http::{HOOK_PATH, TOKEN_ENV};

/// Ownership marker of the retired `y agent-hook` command entries.
pub const LEGACY_MARKER: &str = "--ymux-agent-hook";

/// Env var carrying the pane id into every PTY (set in `pty::session`).
pub const PANE_ENV: &str = "YMUX_PANE_ID";

/// Whether an `allowedHttpHookUrls` item is the one ymux adds: exactly
/// `http://127.0.0.1:<port>/ymux-agent-hook` (see [`hook_url`]). Only that
/// spelling — a user's own wildcard for the same path is theirs to keep.
pub fn is_our_allowed_url(item: &str) -> bool {
    item.strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix(HOOK_PATH))
        .is_some_and(|port| {
            !port.is_empty()
                && port.parse::<u16>().is_ok()
                && port.bytes().all(|b| b.is_ascii_digit())
        })
}

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

/// In a user-level `allowedHttpHookUrls` that already exists (never created:
/// defining the key blocks every http hook not on it), allow exactly
/// `url`, dropping ymux's entry for any other port. The exact URL rather than
/// a port wildcard, so ymux never loosens the user's own restriction.
fn allow_exact_url(root: &mut Map<String, Value>, url: &str) {
    if let Some(list) = root.get_mut(ALLOW_URLS_KEY).and_then(Value::as_array_mut) {
        list.retain(|v| {
            !v.as_str()
                .is_some_and(|s| s != url && is_our_allowed_url(s))
        });
        if !list.iter().any(|v| v.as_str() == Some(url)) {
            list.push(Value::String(url.to_string()));
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
    allow_exact_url(root, &hook_url(port));
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
    let mut changed = false;
    if let Some(list) = root.get_mut(ALLOW_URLS_KEY).and_then(Value::as_array_mut) {
        let before = list.len();
        list.retain(|v| !v.as_str().is_some_and(is_our_allowed_url));
        changed |= list.len() != before;
    }
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

/// `<dir>/settings.json`: Claude Code's user-level settings for the config
/// home `dir` (`CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn settings_file(dir: &str) -> PathBuf {
    Path::new(dir).join("settings.json")
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

/// Atomically and durably replace `target` with `text` ([`crate::durable`]:
/// unique sibling temp file, `sync_all`, rename). The new file keeps
/// `target`'s permissions — a user's `0600` `settings.json` must not come
/// back `0644` — and the temp file never outlives a failure.
fn replace_file(target: &Path, text: &str) -> std::io::Result<()> {
    let perms = match fs::metadata(target) {
        Ok(m) => Some(m.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    crate::durable::write_atomic_with(target, text.as_bytes(), perms.as_ref())
}

/// `settings.json.ymux-bak`: the user's file as it was before ymux first
/// wrote to it. Kept while ymux's hooks are installed, removed by a
/// successful uninstall (it holds whatever the settings held, secrets too).
fn backup_path(path: &Path) -> PathBuf {
    sibling(path, "ymux-bak")
}

/// Copy the original to [`backup_path`] with the original's permissions
/// (Unix: applied before any byte is written; on Windows the read-only flag
/// is not copied, so the backup can always be deleted later).
fn write_backup(path: &Path, backup: &Path) -> std::io::Result<()> {
    let bytes = fs::read(path)?;
    #[cfg(unix)]
    let perms = Some(fs::metadata(path)?.permissions());
    #[cfg(not(unix))]
    let perms: Option<fs::Permissions> = None;
    crate::durable::write_atomic_with(backup, &bytes, perms.as_ref())
}

/// Atomic write (temp file + rename) through symlinks. Before the first write
/// ever, the original is copied to `settings.json.ymux-bak`.
fn write_settings(path: &Path, value: &Value) -> YmuxResult<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = backup_path(path);
    if path.exists() && !backup.exists() {
        write_backup(path, &backup)?;
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
/// Once ymux's hooks are gone, the backup is deleted too.
pub fn uninstall_at(path: &Path) -> YmuxResult<()> {
    if let Some(mut value) = read_settings(path)? {
        if uninstall_hooks(&mut value) {
            write_settings(path, &value)?;
        }
    }
    remove_backup(path);
    Ok(())
}

/// Best-effort delete of [`backup_path`]; a missing backup is fine, and a
/// failure is only logged (the uninstall itself has succeeded).
fn remove_backup(path: &Path) {
    let backup = backup_path(path);
    match fs::remove_file(&backup) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(path = %backup.display(), error = %e, "removing the settings backup failed")
        }
    }
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

/// Claude config dirs that should carry the hooks: every account's, plus
/// home (the "none" account) and the default (whatever ymux inherited).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredDirs {
    /// Distinct dirs (rule 15), default first.
    pub all: Vec<String>,
    /// The default and home dirs: the only ones a failure blocks the
    /// tracking toggle for, and the only ones installing may create.
    pub required: Vec<String>,
}

impl DesiredDirs {
    pub fn is_required(&self, dir: &str) -> bool {
        self.required.iter().any(|r| ypath::same_path(r, dir))
    }
}

pub fn desired_dirs(
    accounts: &[AccountProfile],
    inherited: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> DesiredDirs {
    let all = crate::accounts::account_dirs(accounts, inherited, home);
    let required = all.iter().take(2).map(|(_, d)| d.claude.clone()).collect();
    let all = crate::accounts::distinct(all.into_iter().map(|(owner, d)| (owner, d.claude)))
        .into_iter()
        .map(|g| g.dir)
        .collect();
    DesiredDirs { all, required }
}

/// What a reconcile should do, both lists distinct by `ypath` key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookPlan {
    pub uninstall: Vec<String>,
    pub install: Vec<String>,
}

impl HookPlan {
    pub fn is_empty(&self) -> bool {
        self.uninstall.is_empty() && self.install.is_empty()
    }
}

fn dedup_by_key(dirs: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for d in dirs {
        let key = ypath::comparison_key(&d);
        if !d.is_empty() && !seen.contains(&key) {
            seen.push(key);
            out.push(d);
        }
    }
    out
}

/// Whether `dir` is in `list`, compared with `ypath` (rule 15).
pub fn has_dir(list: &[String], dir: &str) -> bool {
    let key = ypath::comparison_key(dir);
    list.iter().any(|d| ypath::comparison_key(d) == key)
}

/// Whether `a` and `b` name one directory: equal by `ypath` (rule 15), or,
/// when the filesystem can answer, the same canonical location (symlinks,
/// junctions, 8.3 names). A path that no longer exists falls back to `ypath`.
pub fn same_location(a: &str, b: &str) -> bool {
    if ypath::same_path(a, b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Uninstall from `installed − desired`; install into `desired` (all of it
/// when `refresh_all`, e.g. the port may have changed, else only
/// `desired − installed`). Pure `ypath` comparison; see [`plan_hooks_with`].
pub fn plan_hooks(desired: &[String], installed: &[String], refresh_all: bool) -> HookPlan {
    plan_hooks_with(desired, installed, refresh_all, |a, b| {
        ypath::same_path(a, b)
    })
}

/// [`plan_hooks`] with an injected location test. An installed dir that
/// resolves to the same location as any still-desired dir (an alias spelling)
/// is never uninstalled: that would strip the hooks the desired dir needs.
pub fn plan_hooks_with(
    desired: &[String],
    installed: &[String],
    refresh_all: bool,
    same_location: impl Fn(&str, &str) -> bool,
) -> HookPlan {
    HookPlan {
        uninstall: dedup_by_key(
            installed
                .iter()
                .filter(|d| !has_dir(desired, d) && !desired.iter().any(|w| same_location(w, d)))
                .cloned(),
        ),
        install: dedup_by_key(
            desired
                .iter()
                .filter(|d| refresh_all || !has_dir(installed, d))
                .cloned(),
        ),
    }
}

/// How one directory fared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirResult {
    Done,
    /// Nothing written on purpose (a profile directory that does not exist).
    Skipped,
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanOutcome {
    pub uninstall: Vec<(String, DirResult)>,
    pub install: Vec<(String, DirResult)>,
}

/// A directory whose hooks could not be written or removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HookDirIssue {
    pub dir: String,
    pub error: String,
}

impl PlanOutcome {
    pub fn issues(&self) -> Vec<HookDirIssue> {
        self.uninstall
            .iter()
            .chain(&self.install)
            .filter_map(|(dir, r)| match r {
                DirResult::Failed(error) => Some(HookDirIssue {
                    dir: dir.clone(),
                    error: error.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

/// The dirs that carry hooks after `outcome`: a failed uninstall stays
/// pending (still recorded), a failed or skipped install is not added.
pub fn next_installed(installed: &[String], outcome: &PlanOutcome) -> Vec<String> {
    let removed: Vec<&String> = outcome
        .uninstall
        .iter()
        .filter(|(_, r)| *r == DirResult::Done)
        .map(|(d, _)| d)
        .collect();
    let mut out = dedup_by_key(
        installed
            .iter()
            .filter(|d| !removed.iter().any(|r| ypath::same_path(r, d)))
            .cloned(),
    );
    for (dir, r) in &outcome.uninstall {
        if matches!(r, DirResult::Failed(_)) && !has_dir(&out, dir) {
            out.push(dir.clone());
        }
    }
    for (dir, r) in &outcome.install {
        if *r == DirResult::Done && !has_dir(&out, dir) {
            out.push(dir.clone());
        }
    }
    out
}

/// Run `plan`: every uninstall first, then every install, so a directory that
/// moves between spellings never ends up without hooks. Per file this is
/// `uninstall_at` / `install_at` (rule 12). One directory failing never stops
/// the rest. Only directories `may_create` accepts are created: a profile
/// directory that does not exist is skipped, not made.
pub fn apply_plan(plan: &HookPlan, port: u16, may_create: impl Fn(&str) -> bool) -> PlanOutcome {
    let mut outcome = PlanOutcome::default();
    for dir in &plan.uninstall {
        // We only ever write absolute dirs; anything else holds nothing of ours.
        let result = if !Path::new(dir).is_absolute() {
            DirResult::Done
        } else {
            match uninstall_at(&settings_file(dir)) {
                Ok(()) => DirResult::Done,
                Err(e) => DirResult::Failed(e.to_string()),
            }
        };
        outcome.uninstall.push((dir.clone(), result));
    }
    for dir in &plan.install {
        let result =
            if !Path::new(dir).is_absolute() || (!Path::new(dir).is_dir() && !may_create(dir)) {
                DirResult::Skipped
            } else {
                match install_at(&settings_file(dir), port) {
                    Ok(()) => DirResult::Done,
                    Err(e) => DirResult::Failed(e.to_string()),
                }
            };
        outcome.install.push((dir.clone(), result));
    }
    outcome
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
                "http://127.0.0.1:41234/ymux-agent-hook"
            ]),
            "the exact URL, never a port wildcard that loosens the user's list"
        );
        assert_eq!(
            v["httpHookAllowedEnvVars"],
            json!(["AUDIT_TOKEN", "YMUX_PANE_ID", "YMUX_HOOK_TOKEN"])
        );
        // An empty list (= "block every http hook") gets ymux added, and
        // uninstall puts it back to empty, not absent.
        let mut empty = json!({ "allowedHttpHookUrls": [] });
        install_hooks(&mut empty, PORT).unwrap();
        assert_eq!(empty["allowedHttpHookUrls"], json!([hook_url(PORT)]));
        assert!(uninstall_hooks(&mut empty));
        assert_eq!(empty, json!({ "allowedHttpHookUrls": [] }));
    }

    #[test]
    fn the_allowlisted_url_follows_the_port() {
        let mut v = foreign();
        install_hooks(&mut v, 1111).unwrap();
        install_hooks(&mut v, PORT).unwrap();
        assert_eq!(
            v["allowedHttpHookUrls"],
            json!(["https://audit.example/*", hook_url(PORT)]),
            "the old port's entry is replaced, not kept beside the new one"
        );
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn only_our_exact_url_counts_as_ours_in_the_allowlist() {
        for foreign_url in [
            "http://127.0.0.1:*/ymux-agent-hook",
            "http://127.0.0.1:9999/audit",
            "http://127.0.0.1:/ymux-agent-hook",
            "http://127.0.0.1:12ab/ymux-agent-hook",
            "https://127.0.0.1:1/ymux-agent-hook",
            "http://localhost:1/ymux-agent-hook",
        ] {
            assert!(!is_our_allowed_url(foreign_url), "{foreign_url}");
        }
        assert!(is_our_allowed_url("http://127.0.0.1:41234/ymux-agent-hook"));
        let mut v = json!({ "allowedHttpHookUrls": ["http://127.0.0.1:*/ymux-agent-hook"] });
        install_hooks(&mut v, PORT).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            v,
            json!({ "allowedHttpHookUrls": ["http://127.0.0.1:*/ymux-agent-hook"] }),
            "a user's own wildcard survives"
        );
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
        assert_no_temp_files(path.parent().unwrap());
    }

    /// No `*.tmp` (durable-write temp file) left in `dir`.
    fn assert_no_temp_files(dir: &Path) {
        let left: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "temp files left behind: {left:?}");
    }

    #[test]
    fn first_write_backs_up_the_original_once() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        install_at(&path, PORT).unwrap();
        let bak = sibling(&path, "ymux-bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FOREIGN);
        install_at(&path, 1111).unwrap();
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            FOREIGN,
            "backup never overwritten"
        );
        uninstall_at(&path).unwrap();
        assert!(!bak.exists(), "a successful uninstall deletes the backup");
    }

    #[test]
    fn uninstall_deletes_the_backup_even_when_nothing_changed() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        std::fs::write(sibling(&path, "ymux-bak"), FOREIGN).unwrap();
        uninstall_at(&path).unwrap();
        assert!(!sibling(&path, "ymux-bak").exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), FOREIGN);
    }

    #[cfg(unix)]
    #[test]
    fn rewrite_and_backup_keep_the_original_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        install_at(&path, PORT).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "settings.json");
        assert_eq!(mode(&sibling(&path, "ymux-bak")), 0o600, "backup");
        uninstall_at(&path).unwrap();
        assert_eq!(mode(&path), 0o600, "settings.json after uninstall");
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
        assert_no_temp_files(target.parent().unwrap());
        assert!(target.is_dir(), "target untouched");
    }

    #[test]
    fn replace_file_writes_the_content() {
        let target = tempdir().join("settings.json");
        std::fs::write(&target, "old").unwrap();
        replace_file(&target, "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
        assert_no_temp_files(target.parent().unwrap());
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

    fn s(p: &Path) -> String {
        p.display().to_string()
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn compact(v: &Value) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn plan_installs_each_distinct_dir_once() {
        let plan = plan_hooks(&strs(&["/srv/a", "/srv/a/", "/srv/b"]), &[], false);
        assert_eq!(plan.install, strs(&["/srv/a", "/srv/b"]));
        assert!(plan.uninstall.is_empty());
        // Already installed (another spelling) is not installed again.
        let plan = plan_hooks(&strs(&["/srv/a", "/srv/b"]), &strs(&["/srv/a/"]), false);
        assert_eq!(plan.install, strs(&["/srv/b"]));
        assert!(plan.uninstall.is_empty());
        // Stale dirs are uninstalled once.
        let plan = plan_hooks(&strs(&["/srv/a"]), &strs(&["/srv/x", "/srv/x/"]), false);
        assert_eq!(plan.uninstall, strs(&["/srv/x"]));
    }

    #[test]
    fn deleting_profile_aliasing_home_keeps_home_installed() {
        let home = tempdir();
        let alias = format!("{}/", home.join(".claude").display());
        let profile = AccountProfile {
            id: "p".into(),
            name: "p".into(),
            claude_config_dir: alias,
            codex_home: String::new(),
            color: String::new(),
        };
        let with = desired_dirs(&[profile], |_| None, Some(&home));
        let without = desired_dirs(&[], |_| None, Some(&home));
        assert_eq!(with.all, without.all, "alias folds into home");
        let plan = plan_hooks(&without.all, &with.all, false);
        assert!(plan.is_empty(), "{plan:?}");
        assert!(without.is_required(&s(&home.join(".claude"))));
    }

    #[test]
    fn refresh_all_reinstalls_every_desired_dir() {
        let plan = plan_hooks(&strs(&["/srv/a", "/srv/b"]), &strs(&["/srv/a"]), true);
        assert_eq!(plan.install, strs(&["/srv/a", "/srv/b"]));
        assert!(plan.uninstall.is_empty());
    }

    #[test]
    fn failed_uninstall_of_unrecorded_dir_becomes_pending() {
        let outcome = PlanOutcome {
            uninstall: vec![("/srv/x".into(), DirResult::Failed("busy".into()))],
            install: vec![],
        };
        assert_eq!(next_installed(&[], &outcome), strs(&["/srv/x"]));
    }

    #[test]
    fn alias_of_a_desired_dir_is_never_uninstalled() {
        let same = |a: &str, b: &str| a.trim_end_matches('~') == b.trim_end_matches('~');
        let plan = plan_hooks_with(
            &strs(&["/srv/real"]),
            &strs(&["/srv/real~", "/srv/other"]),
            false,
            same,
        );
        assert_eq!(plan.uninstall, strs(&["/srv/other"]));
    }

    #[test]
    fn symlinked_alias_of_a_desired_dir_is_never_uninstalled() {
        let base = tempdir();
        let real = base.join("real");
        let link = base.join("link");
        std::fs::create_dir_all(&real).expect("mkdir");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let made = false;
        if !made {
            return; // link creation not permitted here
        }
        assert!(same_location(&s(&real), &s(&link)));
        let plan = plan_hooks_with(&[s(&real)], &[s(&link)], false, same_location);
        assert!(plan.uninstall.is_empty(), "{plan:?}");
    }

    #[test]
    fn next_installed_keeps_failed_uninstalls_pending() {
        let installed = strs(&["/srv/a", "/srv/b", "/srv/c"]);
        let outcome = PlanOutcome {
            uninstall: vec![
                ("/srv/b".into(), DirResult::Done),
                ("/srv/c".into(), DirResult::Failed("busy".into())),
            ],
            install: vec![
                ("/srv/d".into(), DirResult::Done),
                ("/srv/e".into(), DirResult::Failed("bad".into())),
                ("/srv/f".into(), DirResult::Skipped),
                ("/srv/a/".into(), DirResult::Done),
            ],
        };
        assert_eq!(
            next_installed(&installed, &outcome),
            strs(&["/srv/a", "/srv/c", "/srv/d"])
        );
        let issues = outcome.issues();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].dir, "/srv/c");
    }

    #[test]
    fn apply_plan_round_trips_foreign_settings_in_every_dir() {
        let (a, b) = (tempdir(), tempdir());
        for d in [&a, &b] {
            std::fs::write(settings_file(&s(d)), FOREIGN).unwrap();
        }
        let dirs = vec![s(&a), s(&b)];
        let out = apply_plan(&plan_hooks(&dirs, &[], false), PORT, |_| false);
        assert!(out.install.iter().all(|(_, r)| *r == DirResult::Done));
        for d in &dirs {
            let v = read(&settings_file(d));
            for (event, _) in HOOK_EVENTS {
                assert_eq!(ours_in(&v, event), vec![hook_entry(PORT)], "{d} {event}");
            }
        }
        let installed = next_installed(&[], &out);
        assert_eq!(installed, dirs);
        let out = apply_plan(&plan_hooks(&[], &installed, false), PORT, |_| false);
        assert!(out.uninstall.iter().all(|(_, r)| *r == DirResult::Done));
        for d in &dirs {
            let file = settings_file(d);
            assert_eq!(compact(&read(&file)), compact(&foreign()), "{d}");
            assert!(
                !sibling(&file, "ymux-bak").exists(),
                "a successful uninstall deletes the backup"
            );
        }
        assert!(next_installed(&installed, &out).is_empty());
    }

    #[test]
    fn skips_missing_profile_dir_without_creating_it() {
        let base = tempdir();
        let profile = s(&base.join("profile"));
        let home = s(&base.join("home-claude"));
        let plan = plan_hooks(&[profile.clone(), home.clone()], &[], false);
        let h = home.clone();
        let out = apply_plan(&plan, PORT, |d| d == h);
        assert_eq!(out.install[0], (profile.clone(), DirResult::Skipped));
        assert_eq!(out.install[1], (home.clone(), DirResult::Done));
        assert!(!Path::new(&profile).exists(), "profile dir not created");
        assert!(settings_file(&home).exists(), "home dir may be created");
        assert_eq!(next_installed(&[], &out), vec![home]);
    }

    #[test]
    fn uninstall_of_a_missing_dir_is_success() {
        let gone = s(&tempdir().join("nope"));
        let plan = HookPlan {
            uninstall: vec![gone.clone()],
            install: vec![],
        };
        let out = apply_plan(&plan, PORT, |_| false);
        assert_eq!(out.uninstall, vec![(gone, DirResult::Done)]);
    }

    #[test]
    fn uninstalls_before_installs() {
        let d = tempdir();
        let dir = s(&d);
        install_at(&settings_file(&dir), 1111).unwrap();
        // The same dir on both lists: only "uninstall, then install" leaves
        // hooks behind, and at the new port.
        let plan = HookPlan {
            uninstall: vec![dir.clone()],
            install: vec![dir.clone()],
        };
        apply_plan(&plan, PORT, |_| false);
        assert_eq!(
            ours_in(&read(&settings_file(&dir)), "Stop"),
            vec![hook_entry(PORT)]
        );
    }

    #[test]
    fn bad_settings_in_one_dir_does_not_stop_others() {
        let (bad, good) = (tempdir(), tempdir());
        std::fs::write(settings_file(&s(&bad)), "{ not json").unwrap();
        let plan = plan_hooks(&[s(&bad), s(&good)], &[], false);
        let out = apply_plan(&plan, PORT, |_| false);
        assert!(matches!(out.install[0].1, DirResult::Failed(_)));
        assert_eq!(out.install[1].1, DirResult::Done);
        assert_eq!(
            std::fs::read_to_string(settings_file(&s(&bad))).unwrap(),
            "{ not json"
        );
        assert!(settings_file(&s(&good)).exists());
        assert_eq!(next_installed(&[], &out), vec![s(&good)]);
        assert_eq!(out.issues().len(), 1);
    }
}
