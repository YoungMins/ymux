use std::{path::PathBuf, time::Duration};

use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::quota_cache::REFRESH;
use super::{
    model::Quota,
    quota_cache::{read_bounded, Cached, Unavailable},
    TokenSnapshot,
};

#[cfg(target_os = "macos")]
const MAX_BYTES: u64 = 1024 * 1024;
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

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
    let Some(provider) = snapshot.providers.iter_mut().find(|p| p.id == "claude") else {
        return;
    };
    let Some(token) = credentials() else {
        *CACHE.lock() = None;
        provider.quotas.clear();
        return;
    };
    provider.auth_status = "credentials_present";
    let identity: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    // This synchronous adapter runs on a blocking thread; serialize refreshes to
    // prevent concurrent panel requests from bypassing the rate-limit backoff.
    let mut cache = CACHE.lock();
    let needs_refresh = cache
        .as_ref()
        .map_or(true, |c| c.needs_refresh(identity, manual));
    if needs_refresh {
        let result = fetch(&token);
        *cache = Some(Cached::refreshed(cache.take(), identity, result));
    }
    if let Some(cached) = cache.as_ref() {
        provider.quotas = cached.quotas.clone();
        if cached.unavailable {
            snapshot.warnings.push(
                if cached.quotas.is_empty() {
                    "claude_quota_unavailable"
                } else {
                    "claude_quota_stale"
                }
                .into(),
            );
        }
    }
}

fn credentials() -> Option<String> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    let file_token = std::fs::File::open(root.join(".credentials.json"))
        .ok()
        .and_then(|file| read_bounded(file).ok())
        .and_then(|bytes| parse_credentials(&bytes));
    if file_token.is_some() {
        return file_token;
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .ok()?;
        if output.status.success() && u64::try_from(output.stdout.len()).ok()? <= MAX_BYTES {
            return parse_credentials(&output.stdout);
        }
    }
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
}
