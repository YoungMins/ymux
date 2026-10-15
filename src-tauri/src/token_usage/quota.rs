use std::{path::Path, sync::OnceLock, time::Duration};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::quota_cache::{Cached, REFRESH};
use super::{
    model::Quota,
    quota_cache::{read_bounded, round, Identity, KeyedCache, Unavailable},
    TokenSnapshot,
};

#[cfg(target_os = "macos")]
const MAX_BYTES: u64 = 1024 * 1024;
static CACHE: OnceLock<KeyedCache> = OnceLock::new();

#[derive(Deserialize)]
struct Credentials {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}

#[derive(Deserialize)]
struct OAuth {
    #[serde(rename = "accessToken")]
    access_token: String,
}

#[derive(Deserialize)]
struct Usage {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    utilization: f64,
    resets_at: Option<String>,
}

/// Adds account quota percentages without exposing credentials or network errors.
pub fn enrich(snapshot: &mut TokenSnapshot, manual: bool) {
    enrich_claude(snapshot, manual);
    super::codex_quota::enrich(snapshot, manual);
}

fn enrich_claude(snapshot: &mut TokenSnapshot, manual: bool) {
    // Each Claude card reads its own directory's login; cards that resolve to
    // one login share a single fetch.
    let mut jobs: Vec<(Identity, String)> = Vec::new();
    let mut identities: Vec<Option<Identity>> = Vec::new();
    for provider in snapshot.providers.iter_mut().filter(|p| p.id == "claude") {
        let Some(token) = credentials_in(&provider.dir, provider.claude_env.as_deref()) else {
            provider.quotas.clear();
            identities.push(None);
            continue;
        };
        provider.auth_status = "credentials_present";
        let identity: Identity = Sha256::digest(token.as_bytes()).into();
        jobs.push((identity, token));
        identities.push(Some(identity));
    }
    let cache = CACHE.get_or_init(Default::default);
    let results = round(cache, jobs, manual, |token| fetch(token));
    let mut identities = identities.into_iter();
    for provider in snapshot.providers.iter_mut().filter(|p| p.id == "claude") {
        let Some(Some(identity)) = identities.next() else {
            continue;
        };
        if let Some((quotas, unavailable)) = results.get(&identity) {
            provider.quotas = quotas.clone();
            if *unavailable {
                provider.warnings.push(if quotas.is_empty() {
                    "quota_unavailable"
                } else {
                    "quota_stale"
                });
            }
        }
    }
}

/// The OAuth token for the Claude config home `dir`, from its
/// `.credentials.json` or the macOS keychain.
fn credentials_in(dir: &Path, claude_env: Option<&str>) -> Option<String> {
    credentials_with(dir, claude_env, security_lookup)
}

/// `credentials_in` with the keychain lookup injected. Fails closed: the
/// service name is derived from the account's own `CLAUDE_CONFIG_DIR`, and a
/// set variable never falls back to the unsuffixed default service (that
/// would show another login's quota on a profile card).
fn credentials_with(
    dir: &Path,
    claude_env: Option<&str>,
    keychain: impl Fn(&str) -> Option<Vec<u8>>,
) -> Option<String> {
    let file_token = std::fs::File::open(dir.join(".credentials.json"))
        .ok()
        .and_then(|file| read_bounded(file).ok())
        .and_then(|bytes| parse_credentials(&bytes));
    if file_token.is_some() {
        return file_token;
    }
    keychain(&crate::accounts::claude_keychain_service(claude_env))
        .and_then(|bytes| parse_credentials(&bytes))
}

#[cfg(target_os = "macos")]
fn security_lookup(service: &str) -> Option<Vec<u8>> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .ok()?;
    (output.status.success() && u64::try_from(output.stdout.len()).ok()? <= MAX_BYTES)
        .then_some(output.stdout)
}

#[cfg(not(target_os = "macos"))]
fn security_lookup(_service: &str) -> Option<Vec<u8>> {
    None
}

fn parse_credentials(bytes: &[u8]) -> Option<String> {
    let token = serde_json::from_slice::<Credentials>(bytes)
        .ok()?
        .oauth?
        .access_token;
    if token.trim().is_empty() {
        None
    } else {
        Some(token)
    }
}

fn fetch(token: &str) -> Result<Vec<Quota>, Unavailable> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Unavailable)?;
    let response = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| Unavailable)?;
    let bytes = read_bounded(response)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_err(|_| Unavailable)?
        .as_secs();
    parse_usage(&bytes, now)
}

fn parse_usage(bytes: &[u8], now: u64) -> Result<Vec<Quota>, Unavailable> {
    let usage: Usage = serde_json::from_slice(bytes).map_err(|_| Unavailable)?;
    let mut quotas = Vec::new();
    for (minutes, window) in [(300, usage.five_hour), (10080, usage.seven_day)] {
        let Some(window) = window else {
            continue;
        };
        if !window.utilization.is_finite() || !(0.0..=100.0).contains(&window.utilization) {
            return Err(Unavailable);
        }
        let resets_at = match window.resets_at {
            Some(value) => {
                Some(crate::agent_scan_disk::parse_rfc3339_secs(&value).ok_or(Unavailable)?)
            }
            None => None,
        };
        quotas.push(Quota {
            window_minutes: minutes,
            used_percent: window.utilization,
            resets_at,
            observed_at: now,
        });
    }
    if quotas.is_empty() {
        return Err(Unavailable);
    }
    Ok(quotas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_windows_parse_percentages_and_reset_times() {
        let bytes = br#"{"five_hour":{"utilization":25.5,"resets_at":"2026-01-01T00:00:00Z"},"seven_day":{"utilization":80,"resets_at":null}}"#;
        let quotas = parse_usage(bytes, 42).expect("valid usage");
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].used_percent, 25.5);
        assert_eq!(quotas[0].resets_at, Some(1767225600));
        assert_eq!(quotas[1].window_minutes, 10080);
        assert_eq!(quotas[1].observed_at, 42);
    }

    #[test]
    fn malformed_or_out_of_range_account_usage_is_unavailable() {
        for bytes in [
            br#"{}"#.as_slice(),
            br#"{"five_hour":{"utilization":101,"resets_at":null}}"#,
            br#"{"five_hour":{"utilization":20,"resets_at":"invalid"}}"#,
        ] {
            assert!(parse_usage(bytes, 42).is_err());
        }
    }

    #[test]
    fn credentials_require_nonempty_oauth_token() {
        assert!(parse_credentials(br#"{"claudeAiOauth":{"accessToken":" "}}"#).is_none());
        assert_eq!(
            parse_credentials(br#"{"claudeAiOauth":{"accessToken":"example"}}"#).as_deref(),
            Some("example")
        );
    }

    #[test]
    fn reads_reject_oversized_payloads() {
        assert!(read_bounded(std::io::repeat(0)).is_err());
    }

    #[test]
    fn failed_refresh_preserves_only_the_same_accounts_quota() {
        let make = || {
            Cached::refreshed(
                None,
                [1; 32],
                Ok(vec![Quota {
                    window_minutes: 300,
                    used_percent: 20.0,
                    resets_at: None,
                    observed_at: 42,
                }]),
            )
        };
        let same = Cached::refreshed(Some(make()), [1; 32], Err(Unavailable));
        assert_eq!(same.quotas[0].observed_at, 42);
        assert!(same.unavailable);
        assert!(same.attempted_at.elapsed() < REFRESH);
        let different = Cached::refreshed(Some(make()), [2; 32], Err(Unavailable));
        assert!(different.quotas.is_empty());
    }

    #[test]
    fn profile_roots_never_query_unsuffixed_keychain() {
        use crate::accounts::{account_dirs, claude_keychain_service};
        use crate::config::model::AccountProfile;
        use std::cell::RefCell;
        let home = std::path::PathBuf::from(if cfg!(windows) {
            r"C:\Users\a"
        } else {
            "/home/a"
        });
        let profile = home.join(".claude-work").display().to_string();
        let accounts = vec![AccountProfile {
            id: "work".into(),
            name: "Work".into(),
            claude_config_dir: profile.clone(),
            ..AccountProfile::default()
        }];
        let dirs = account_dirs(&accounts, |_| None, Some(&home));
        let roots = super::super::scan_roots(&dirs);
        let asked = RefCell::new(Vec::new());
        let lookup = |service: &str| {
            asked.borrow_mut().push(service.to_owned());
            None
        };
        let tmp = tempfile::tempdir().unwrap();
        for root in roots.iter().filter(|r| r.provider == "claude") {
            asked.borrow_mut().clear();
            assert!(credentials_with(tmp.path(), root.claude_env.as_deref(), lookup).is_none());
            let asked = asked.borrow();
            assert_eq!(asked.len(), 1);
            if root.owner == "work" {
                assert_eq!(root.claude_env.as_deref(), Some(profile.as_str()));
                assert_ne!(asked[0], "Claude Code-credentials");
                assert_eq!(asked[0], claude_keychain_service(Some(&profile)));
            } else {
                assert_eq!(asked[0], "Claude Code-credentials");
            }
        }
        assert!(roots
            .iter()
            .any(|r| r.owner == "work" && r.provider == "claude"));
    }
}
