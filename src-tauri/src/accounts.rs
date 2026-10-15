//! Per-pane AI-CLI accounts: turn "this pane / workspace uses account X" into
//! the environment variables that point Claude Code and Codex at that
//! account's config directory (pure, unit-tested; the spawn path calls it).
//!
//! Only newly spawned PTYs see the result: an env var cannot be changed on a
//! running process, so a live pane keeps its account until it is respawned.
//!
//! The same variables decide where the agents keep their data, so
//! [`agent_dirs`] derives the directories a pane's agents really use from
//! exactly what the spawn applies (`PtyManager` records it per pane). Session
//! resume and the process scan read it, token usage scans each account's dirs, and
//! the Claude Code hooks installer (`agent_hooks.rs`) writes into every
//! account's Claude dir.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::agent_sessions::AgentKind;
use crate::config::model::AccountProfile;

/// `PaneSpec::account` value meaning "no account, even if the workspace has
/// one". Profile ids are generated hex slugs, so they can never collide.
pub const ACCOUNT_NONE: &str = "none";

pub const CLAUDE_ENV: &str = "CLAUDE_CONFIG_DIR";
pub const CODEX_ENV: &str = "CODEX_HOME";

/// Expand a leading `~` (`~`, `~/x`, `~\x`) against `home`. Anything else is
/// returned as written. `None` when a `~` path has no home to expand against.
fn expand_tilde(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    if raw == "~" {
        return home.map(Path::to_path_buf);
    }
    match raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Some(rest) => home.map(|h| h.join(rest)),
        None => Some(PathBuf::from(raw)),
    }
}

/// The profile a pane runs under, or `None` for "no account". A pane's own
/// non-empty choice wins over the workspace default; `ACCOUNT_NONE` opts out;
/// an id that matches no profile injects nothing.
pub fn effective_account<'a>(
    accounts: &'a [AccountProfile],
    pane_account: &str,
    ws_account: &str,
) -> Option<&'a AccountProfile> {
    let id = if pane_account.is_empty() {
        ws_account
    } else {
        pane_account
    };
    if id.is_empty() || id == ACCOUNT_NONE {
        return None;
    }
    let found = accounts.iter().find(|a| a.id == id);
    if found.is_none() {
        tracing::warn!(account = id, "pane references an unknown account; ignoring");
    }
    found
}

/// The value a directory variable is given: trimmed, a leading `~` expanded
/// against `home`, and only if the result is absolute (Claude Code rejects a
/// relative `CLAUDE_CONFIG_DIR`). `None` for empty, relative, or `~` without
/// a home.
pub fn injected_value(raw: &str, home: Option<&Path>) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    expand_tilde(raw, home)
        .filter(|p| p.is_absolute())
        .map(|p| p.display().to_string())
}

/// Env vars to inject for the pane's effective account. Empty directories are
/// skipped (that tool stays on its default), and a name present in
/// `explicit_env` (the pane's own `PaneSpec::env`) is never overridden.
pub fn account_env(
    accounts: &[AccountProfile],
    pane_account: &str,
    ws_account: &str,
    home: Option<&Path>,
    explicit_env: &[(String, String)],
) -> Vec<(String, String)> {
    let Some(profile) = effective_account(accounts, pane_account, ws_account) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, raw) in [
        (CLAUDE_ENV, &profile.claude_config_dir),
        (CODEX_ENV, &profile.codex_home),
    ] {
        if raw.trim().is_empty() {
            continue;
        }
        // Windows env names are case-insensitive; be conservative everywhere.
        if explicit_env
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            continue;
        }
        match injected_value(raw, home) {
            Some(value) => out.push((name.to_string(), value)),
            None => tracing::warn!(
                var = name,
                "account directory is not absolute (or `~` has no home); ignoring"
            ),
        }
    }
    out
}

/// Variables to strip from the child's inherited environment: an effective
/// account of `ACCOUNT_NONE` is a hard opt-out, so a `CLAUDE_CONFIG_DIR` /
/// `CODEX_HOME` set in ymux's own environment must not leak into the pane.
/// A name in `explicit_env` (the pane's own `PaneSpec::env`) is kept.
pub fn account_env_removals(
    pane_account: &str,
    ws_account: &str,
    explicit_env: &[(String, String)],
) -> Vec<&'static str> {
    let id = if pane_account.is_empty() {
        ws_account
    } else {
        pane_account
    };
    if id != ACCOUNT_NONE {
        return Vec::new();
    }
    [CLAUDE_ENV, CODEX_ENV]
        .into_iter()
        .filter(|name| {
            !explicit_env
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case(name))
        })
        .collect()
}

/// Create the directories the injected vars point at, so the tool does not
/// trip over a missing one. Failures are logged, never fatal.
pub fn ensure_dirs(env: &[(String, String)]) {
    for (name, dir) in env {
        if !Path::new(dir).is_absolute() {
            tracing::warn!(var = %name, dir = %dir, "account directory is not absolute; not creating it");
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!(var = %name, dir = %dir, error = %e, "cannot create account directory");
        }
    }
}

/// Where a pane's agents keep their data, as raw spellings (for storage and
/// for opening; compare with `ypath`, rule 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDirs {
    /// Claude Code's config home (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub claude: String,
    /// Codex's home (`CODEX_HOME` or `~/.codex`).
    pub codex: String,
    /// The literal `CLAUDE_CONFIG_DIR` the child sees, `None` when unset.
    /// Claude Code hashes it into its macOS keychain service name.
    pub claude_env: Option<String>,
}

impl AgentDirs {
    pub fn for_agent(&self, agent: AgentKind) -> &str {
        match agent {
            AgentKind::Claude => &self.claude,
            AgentKind::Codex => &self.codex,
        }
    }
}

/// Whether two env var names are the same variable on this platform.
fn same_var(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn default_dir(home: Option<&Path>, sub: &str) -> String {
    home.map(|h| h.join(sub).display().to_string())
        .unwrap_or_default()
}

/// The directories of a process started with the environment a pane gets:
/// `removed` names are scrubbed from the inherited environment, then `layers`
/// apply in order (the last layer that sets a name wins, and a set name beats
/// a removal); otherwise the `inherited` value is used. A value that is
/// empty, relative or unexpandable counts as unset, which means `~/.claude` /
/// `~/.codex`.
pub fn agent_dirs(
    removed: &[&str],
    layers: &[&[(String, String)]],
    inherited: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> AgentDirs {
    let resolve = |var: &str| -> Option<String> {
        let set = layers
            .iter()
            .rev()
            .find_map(|l| l.iter().rev().find(|(k, _)| same_var(k, var)));
        let raw = match set {
            Some((_, v)) => v.clone(),
            None if removed.iter().any(|r| same_var(r, var)) => return None,
            None => inherited(var)?,
        };
        injected_value(&raw, home)
    };
    let claude_env = resolve(CLAUDE_ENV);
    AgentDirs {
        claude: claude_env
            .clone()
            .unwrap_or_else(|| default_dir(home, ".claude")),
        codex: resolve(CODEX_ENV).unwrap_or_else(|| default_dir(home, ".codex")),
        claude_env,
    }
}

/// The directories of a process with no account variables at all: the meaning
/// of the "none" account, and of a record written before records carried a
/// directory.
pub fn home_dirs(home: Option<&Path>) -> AgentDirs {
    AgentDirs {
        claude: default_dir(home, ".claude"),
        codex: default_dir(home, ".codex"),
        claude_env: None,
    }
}

/// Every account's directories: `""` (no account: whatever ymux inherited),
/// `"none"` (home, listed even when it equals the default) and each profile.
/// A profile that leaves a tool's directory empty shares the default's
/// directory for it, not home.
pub fn account_dirs(
    accounts: &[AccountProfile],
    inherited: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Vec<(String, AgentDirs)> {
    let mut out = vec![
        (String::new(), agent_dirs(&[], &[], &inherited, home)),
        (ACCOUNT_NONE.to_string(), home_dirs(home)),
    ];
    for p in accounts {
        let env = account_env(accounts, &p.id, "", home, &[]);
        out.push((p.id.clone(), agent_dirs(&[], &[&env], &inherited, home)));
    }
    out
}

/// One directory and who uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirGroup {
    pub dir: String,
    /// The first owner that listed it.
    pub owner: String,
    /// Later, different owners of the same directory.
    pub also: Vec<String>,
}

/// Group `(owner, dir)` pairs by directory, comparing with `ypath` (rule 15)
/// so two spellings of one directory are one group. The first spelling and
/// first owner win; empty directories are dropped.
pub fn distinct(pairs: impl IntoIterator<Item = (String, String)>) -> Vec<DirGroup> {
    let mut out: Vec<(String, DirGroup)> = Vec::new();
    for (owner, dir) in pairs {
        if dir.is_empty() {
            continue;
        }
        let key = ypath::comparison_key(&dir);
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, g)) => {
                if g.owner != owner && !g.also.contains(&owner) {
                    g.also.push(owner);
                }
            }
            None => out.push((
                key,
                DirGroup {
                    dir,
                    owner,
                    also: Vec::new(),
                },
            )),
        }
    }
    out.into_iter().map(|(_, g)| g).collect()
}

/// The macOS keychain service Claude Code 2.1.300 keeps its credentials
/// under: `Claude Code-credentials`, plus `-` and the first 8 hex digits of
/// the SHA-256 of the NFC-normalised literal `CLAUDE_CONFIG_DIR` when that is
/// set. Derived from Claude's internals, so it can drift; callers must fail
/// closed (never fall back to the unsuffixed name for a set variable).
pub fn claude_keychain_service(claude_env: Option<&str>) -> String {
    let mut service = String::from("Claude Code-credentials");
    if let Some(v) = claude_env.filter(|v| !v.is_empty()) {
        let digest = Sha256::digest(ypath::nfc(v).as_bytes());
        service.push('-');
        for b in &digest[..4] {
            let _ = write!(service, "{b:02x}");
        }
    }
    service
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, claude: &str, codex: &str) -> AccountProfile {
        AccountProfile {
            id: id.into(),
            name: id.into(),
            claude_config_dir: claude.into(),
            codex_home: codex.into(),
            color: String::new(),
        }
    }

    /// A platform-absolute path: `/x` is not absolute on Windows.
    fn ab(x: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{x}")
        } else {
            format!("/{x}")
        }
    }

    fn env(
        accounts: &[AccountProfile],
        pane: &str,
        ws: &str,
        explicit: &[(String, String)],
    ) -> Vec<(String, String)> {
        account_env(accounts, pane, ws, Some(Path::new(&ab("h"))), explicit)
    }

    fn kv(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    fn home_join(rest: &str) -> String {
        Path::new(&ab("h")).join(rest).display().to_string()
    }

    #[test]
    fn pane_account_beats_workspace_account() {
        let accts = [profile("a", &ab("ca"), ""), profile("b", &ab("cb"), "")];
        assert_eq!(env(&accts, "b", "a", &[]), [kv(CLAUDE_ENV, &ab("cb"))]);
    }

    #[test]
    fn empty_pane_account_inherits_the_workspace() {
        let accts = [profile("a", &ab("ca"), &ab("xa"))];
        assert_eq!(
            env(&accts, "", "a", &[]),
            [kv(CLAUDE_ENV, &ab("ca")), kv(CODEX_ENV, &ab("xa"))]
        );
    }

    #[test]
    fn none_opts_out_of_the_workspace_account() {
        let accts = [profile("a", &ab("ca"), &ab("xa"))];
        assert!(env(&accts, ACCOUNT_NONE, "a", &[]).is_empty());
    }

    #[test]
    fn no_account_anywhere_injects_nothing() {
        let accts = [profile("a", &ab("ca"), &ab("xa"))];
        assert!(env(&accts, "", "", &[]).is_empty());
    }

    #[test]
    fn unknown_ids_inject_nothing_and_do_not_fall_back() {
        let accts = [profile("a", &ab("ca"), "")];
        assert!(env(&accts, "gone", "a", &[]).is_empty());
        assert!(env(&accts, "", "gone", &[]).is_empty());
    }

    #[test]
    fn empty_dirs_leave_that_tool_on_its_default() {
        let accts = [profile("a", "", &ab("xa")), profile("b", "  ", "")];
        assert_eq!(env(&accts, "a", "", &[]), [kv(CODEX_ENV, &ab("xa"))]);
        assert!(env(&accts, "b", "", &[]).is_empty());
    }

    #[test]
    fn leading_tilde_expands_to_home() {
        let accts = [
            profile("a", "~/.claude-work", "~"),
            profile("b", "~\\c", ""),
        ];
        let got = env(&accts, "a", "", &[]);
        assert_eq!(got[0].1, home_join(".claude-work"));
        assert_eq!(got[1].1, Path::new(&ab("h")).display().to_string());
        assert_eq!(env(&accts, "b", "", &[])[0].1, home_join("c"));
    }

    #[test]
    fn tilde_inside_a_name_is_not_expanded() {
        let mid = ab("a/~/b");
        let accts = [profile("a", "~work/x", &mid)];
        // `~work/x` is not expanded, so it is relative and gets skipped.
        assert_eq!(env(&accts, "a", "", &[]), [kv(CODEX_ENV, &mid)]);
    }

    #[test]
    fn relative_dirs_are_skipped_and_never_created() {
        let accts = [profile("a", "rel/claude", "./codex")];
        assert!(env(&accts, "a", "", &[]).is_empty());
        let rel = format!("ymux-rel-{}", uuid::Uuid::new_v4());
        ensure_dirs(&[(CLAUDE_ENV.into(), rel.clone())]);
        assert!(!Path::new(&rel).exists());
    }

    #[test]
    fn none_removes_inherited_vars_unless_explicit() {
        let both = vec![CLAUDE_ENV, CODEX_ENV];
        assert_eq!(account_env_removals(ACCOUNT_NONE, "a", &[]), both);
        assert_eq!(account_env_removals("", ACCOUNT_NONE, &[]), both);
        assert_eq!(
            account_env_removals(ACCOUNT_NONE, "", &[kv("codex_home", "x")]),
            vec![CLAUDE_ENV]
        );
        assert!(account_env_removals("a", "", &[]).is_empty());
        assert!(account_env_removals("", "", &[]).is_empty());
        assert!(account_env_removals("", "a", &[]).is_empty());
    }

    #[test]
    fn tilde_without_a_home_is_skipped() {
        let accts = [profile("a", "~/x", &ab("abs"))];
        let got = account_env(&accts, "a", "", None, &[]);
        assert_eq!(got, [kv(CODEX_ENV, &ab("abs"))]);
    }

    #[test]
    fn explicit_pane_env_wins() {
        let accts = [profile("a", &ab("ca"), &ab("xa"))];
        let explicit = [kv("claude_config_dir", &ab("mine"))];
        assert_eq!(env(&accts, "a", "", &explicit), [kv(CODEX_ENV, &ab("xa"))]);
    }

    #[test]
    fn ensure_dirs_creates_missing_and_tolerates_failure() {
        let base = std::env::temp_dir().join(format!("ymux-acct-{}", uuid::Uuid::new_v4()));
        let made = base.join("nested").join("claude");
        // A path under a regular file cannot be created; must only warn.
        let file = base.join("file");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&file, b"x").unwrap();
        ensure_dirs(&[
            (CLAUDE_ENV.into(), made.display().to_string()),
            (CODEX_ENV.into(), file.join("sub").display().to_string()),
        ]);
        assert!(made.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn lit(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    fn h() -> PathBuf {
        PathBuf::from(ab("h"))
    }

    fn dot(sub: &str) -> String {
        h().join(sub).display().to_string()
    }

    #[test]
    fn agent_dirs_last_layer_beats_removal_beats_inherited() {
        let first = [kv(CLAUDE_ENV, &ab("one"))];
        let second = [kv(CLAUDE_ENV, &ab("two")), kv(CODEX_ENV, &ab("cx"))];
        let inherit = |k: &str| (k == CLAUDE_ENV).then(|| ab("inh"));
        let home = h();
        // The last layer wins over an earlier one, a removal and the inherited value.
        let d = agent_dirs(&[CLAUDE_ENV], &[&first, &second], inherit, Some(&home));
        assert_eq!(d.claude, ab("two"));
        assert_eq!(d.claude_env, Some(ab("two")));
        assert_eq!(d.codex, ab("cx"));
        // No layer: the removal beats the inherited value (home default, env unset).
        let d = agent_dirs(&[CLAUDE_ENV], &[], inherit, Some(&home));
        assert_eq!(d.claude, dot(".claude"));
        assert_eq!(d.claude_env, None);
        // Neither: the inherited value is used.
        let d = agent_dirs(&[], &[], inherit, Some(&home));
        assert_eq!(d.claude, ab("inh"));
        assert_eq!(d.claude_env, Some(ab("inh")));
        assert_eq!(d.codex, dot(".codex"));
    }

    #[test]
    fn unusable_values_mean_unset() {
        let home = h();
        let rel = [kv(CLAUDE_ENV, "rel/dir")];
        let d = agent_dirs(&[], &[&rel], |_| None, Some(&home));
        assert_eq!(d.claude, dot(".claude"));
        assert_eq!(d.claude_env, None);
        // An empty inherited value is unset; `~` expands against home.
        let d = agent_dirs(
            &[],
            &[],
            |k| if k == CODEX_ENV { lit("~/cx") } else { lit("") },
            Some(&home),
        );
        assert_eq!(d.claude_env, None);
        assert_eq!(d.codex, dot("cx"));
    }

    #[test]
    fn none_removal_falls_back_to_home() {
        let home = h();
        let removed = account_env_removals(ACCOUNT_NONE, "", &[]);
        let inherit = |_: &str| lit(&ab("inh"));
        let d = agent_dirs(&removed, &[], inherit, Some(&home));
        assert_eq!(d, home_dirs(Some(&home)));
        assert_eq!(d.claude, dot(".claude"));
        assert_eq!(d.codex, dot(".codex"));
        assert_eq!(d.claude_env, None);
    }

    #[test]
    fn env_names_fold_case_only_on_windows() {
        let layer = [kv("claude_config_dir", &ab("lc"))];
        let home = h();
        let d = agent_dirs(&[], &[&layer], |_| None, Some(&home));
        if cfg!(windows) {
            assert_eq!(d.claude, ab("lc"));
        } else {
            assert_eq!(d.claude, dot(".claude"), "a different name on POSIX");
        }
    }

    #[test]
    fn profile_without_claude_dir_shares_default_not_home() {
        let home = h();
        let accts = [profile("a", "", &ab("xa")), profile("b", &ab("cb"), "")];
        let inherit = |k: &str| (k == CLAUDE_ENV).then(|| ab("inh"));
        let got = account_dirs(&accts, inherit, Some(&home));
        let by = |id: &str| got.iter().find(|(i, _)| i == id).unwrap().1.clone();
        assert_eq!(by("").claude, ab("inh"));
        assert_eq!(by(ACCOUNT_NONE), home_dirs(Some(&home)));
        // `a` has no Claude dir: it inherits ymux's, not home's.
        assert_eq!(by("a").claude, ab("inh"));
        assert_eq!(by("a").codex, ab("xa"));
        // `b` overrides Claude only.
        assert_eq!(by("b").claude, ab("cb"));
        assert_eq!(by("b").codex, dot(".codex"));
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].0, "");
    }

    #[test]
    fn distinct_folds_spellings_keeps_first_owner() {
        let pairs = [
            ("".to_string(), "/srv/a/".to_string()),
            ("none".to_string(), "/srv/a".to_string()),
            ("p".to_string(), "/srv//a".to_string()),
            ("p".to_string(), "/srv/b".to_string()),
            ("q".to_string(), String::new()),
            ("r".to_string(), "/srv/A".to_string()),
        ];
        let got = distinct(pairs);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].dir, "/srv/a/", "first spelling is kept");
        assert_eq!(got[0].owner, "");
        assert_eq!(got[0].also, ["none", "p"]);
        assert_eq!(
            (got[1].dir.as_str(), got[1].owner.as_str()),
            ("/srv/b", "p")
        );
        assert!(got[1].also.is_empty());
        // POSIX paths are case-sensitive (rule 15).
        assert_eq!(got[2].dir, "/srv/A");
    }

    #[test]
    fn keychain_service_matches_claude_code_2_1_300() {
        assert_eq!(claude_keychain_service(None), "Claude Code-credentials");
        assert_eq!(
            claude_keychain_service(Some("/Users/alice/.claude-work")),
            "Claude Code-credentials-be865d75"
        );
        // The literal value is hashed: a trailing slash is a different one.
        assert_eq!(
            claude_keychain_service(Some("/Users/alice/.claude-work/")),
            "Claude Code-credentials-95672096"
        );
        // NFC first: decomposed Hangul hashes like the composed spelling, not
        // like its raw bytes (which would give 509c162c).
        let nfd = "/Users/alice/\u{1100}\u{1168}\u{110c}\u{1165}\u{11bc}";
        let nfc = "/Users/alice/\u{acc4}\u{c815}";
        assert_ne!(nfd, nfc);
        assert_eq!(
            claude_keychain_service(Some(nfd)),
            "Claude Code-credentials-d3427c34"
        );
        assert_eq!(
            claude_keychain_service(Some(nfc)),
            "Claude Code-credentials-d3427c34"
        );
    }
}
