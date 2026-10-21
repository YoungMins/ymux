//! Lightweight update checker that polls the GitHub releases API and emits a
//! Tauri event when a newer `tag_name` appears upstream.
//!
//! Intentionally notification-only: we never auto-download or auto-install.
//! The frontend decides how to surface the hint — currently a small banner
//! with a "Release notes" link that opens the user's browser.
//!
//! Failure modes (offline, rate-limited, DNS blocked, GitHub down) are all
//! treated as soft: the check logs a warning and the thread sleeps until the
//! next poll. The user experience never degrades because of update checks.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

const POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60); // 6h
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30 * 60); // 30min
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest release JSON read. `releases/latest` is a few KiB plus the notes.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
/// Redirect hops followed, and only within the API host (see
/// [`redirect_allowed`]).
const MAX_REDIRECTS: usize = 3;
const API_HOST: &str = "api.github.com";
const USER_AGENT: &str = concat!("ymux-update-check/", env!("CARGO_PKG_VERSION"));
const RELEASES_URL: &str = "https://api.github.com/repos/YoungMins/ymux/releases/latest";

/// The Tauri event channel the frontend subscribes to. Payload shape is
/// [`UpdateInfo`].
pub const UPDATE_EVENT: &str = "app:update-available";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// Latest upstream version, e.g. `"0.3.1"` (leading `v` stripped).
    pub version: String,
    /// URL of the release page so the banner can link out.
    pub url: String,
    /// Raw release body (markdown). May be empty.
    pub notes: String,
}

/// Spawn the background poller. Returns immediately. Safe to call once per
/// `AppHandle` — additional calls would just spawn duplicate threads.
pub fn start_update_checker(app: AppHandle) {
    std::thread::Builder::new()
        .name("ymux-update-check".into())
        .spawn(move || {
            // Small initial delay so the check never blocks the UI's first paint.
            std::thread::sleep(Duration::from_secs(10));
            loop {
                match check_once() {
                    Ok(Some(info)) => {
                        if let Err(e) = app.emit(UPDATE_EVENT, &info) {
                            tracing::warn!(error = %e, "emit update event failed");
                        } else {
                            tracing::info!(new_version = %info.version, "update available");
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Ok(None) => {
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "update check failed");
                        std::thread::sleep(RETRY_AFTER_FAILURE);
                    }
                }
            }
        })
        .expect("spawn update checker");
}

/// One check iteration. Returns `Ok(Some)` when a newer version is available,
/// `Ok(None)` when nothing newer, and `Err` on transient failures.
fn check_once() -> Result<Option<UpdateInfo>, String> {
    // Test/CI override: lets us verify the banner end-to-end without hitting
    // GitHub. Pattern `version=url=notes` (url/notes optional).
    if let Ok(raw) = std::env::var("YMUX_FAKE_LATEST") {
        return Ok(parse_fake_latest(&raw));
    }

    let current = env!("CARGO_PKG_VERSION");
    // Rule 17's outbound limits: connect + total timeouts, a capped body, and
    // redirects only within the API host. The releases API answers JSON
    // directly; the one redirect it can send is a renamed or transferred
    // repository's 301 to `api.github.com/repositories/<id>/…`.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if redirect_allowed(attempt.url(), attempt.previous().len()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.without_url().to_string())?;

    if !resp.status().is_success() {
        return Err(format!("unexpected status: {}", resp.status()));
    }

    let mut body = Vec::new();
    {
        use std::io::Read;
        resp.take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
    }
    let release = parse_release(&body)?;
    let tag = release.tag_name.trim_start_matches('v');

    if !is_newer(current, tag) {
        return Ok(None);
    }

    Ok(Some(UpdateInfo {
        version: tag.to_string(),
        url: release.html_url,
        notes: release.body.unwrap_or_default(),
    }))
}

/// Whether the update check follows a redirect to `next` after `hops`
/// earlier ones: https on the API host only, a few hops at most.
fn redirect_allowed(next: &reqwest::Url, hops: usize) -> bool {
    hops < MAX_REDIRECTS
        && next.scheme() == "https"
        && next.host_str() == Some(API_HOST)
        && next.port_or_known_default() == Some(443)
}

/// The release JSON, refused when larger than [`MAX_RESPONSE_BYTES`] (the
/// caller reads at most one byte past the cap).
fn parse_release(body: &[u8]) -> Result<GithubRelease, String> {
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("release response too large".into());
    }
    serde_json::from_slice(body).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

fn parse_fake_latest(raw: &str) -> Option<UpdateInfo> {
    let parts: Vec<&str> = raw.splitn(3, '=').collect();
    let version = parts.first()?.trim_start_matches('v').to_string();
    if version.is_empty() {
        return None;
    }
    let url = parts
        .get(1)
        .copied()
        .unwrap_or("https://github.com/YoungMins/ymux/releases")
        .to_string();
    let notes = parts.get(2).copied().unwrap_or("").to_string();
    if !is_newer(env!("CARGO_PKG_VERSION"), &version) {
        return None;
    }
    Some(UpdateInfo {
        version,
        url,
        notes,
    })
}

/// Compare two dotted-numeric version strings, returning `true` if `upstream`
/// is strictly newer than `local`. Pre-release suffixes (`-rc1`, `-beta`) are
/// ignored — if a pre-release tag ships to `releases/latest` we still treat
/// it as a new version.
fn is_newer(local: &str, upstream: &str) -> bool {
    let a = parse_version(local);
    let b = parse_version(upstream);
    b > a
}

fn parse_version(s: &str) -> Vec<u32> {
    let stripped = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    // Take only the leading dotted-numeric run; anything after the first
    // non-digit-non-dot (e.g. "-beta") is ignored.
    let numeric_prefix: String = stripped
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    numeric_prefix
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_detects_patch_bump() {
        assert!(is_newer("0.2.1", "0.2.2"));
        assert!(is_newer("0.2.1", "0.3.0"));
        assert!(is_newer("0.2.1", "1.0.0"));
    }

    #[test]
    fn is_newer_rejects_same_or_older() {
        assert!(!is_newer("0.2.1", "0.2.1"));
        assert!(!is_newer("0.3.0", "0.2.9"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn parse_version_ignores_v_prefix_and_suffixes() {
        assert_eq!(parse_version("v0.3.1"), vec![0, 3, 1]);
        assert_eq!(parse_version("0.3.1-beta"), vec![0, 3, 1]);
        assert_eq!(parse_version("0.3"), vec![0, 3]);
    }

    #[test]
    fn fake_latest_respects_newer_check() {
        // Can't easily set env vars in tests without poisoning other tests;
        // so drive `parse_fake_latest` directly. For a local version of
        // "0.2.1" (set by Cargo), "0.3.0" should yield Some.
        if env!("CARGO_PKG_VERSION") == "0.2.1" || env!("CARGO_PKG_VERSION") < "99.0.0" {
            let info = parse_fake_latest("99.0.0=https://example.com=hello");
            assert!(info.is_some());
            let info = info.unwrap();
            assert_eq!(info.version, "99.0.0");
            assert_eq!(info.url, "https://example.com");
            assert_eq!(info.notes, "hello");
        }
    }

    #[test]
    fn redirects_stay_on_the_api_host() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        let moved = u("https://api.github.com/repositories/1/releases/latest");
        assert!(redirect_allowed(&moved, 0));
        assert!(redirect_allowed(&moved, MAX_REDIRECTS - 1));
        assert!(!redirect_allowed(&moved, MAX_REDIRECTS));
        for bad in [
            "http://api.github.com/x",
            "https://api.github.com:8443/x",
            "https://evil.example/x",
            "https://api.github.com.evil.example/x",
            "https://github.com/x",
        ] {
            assert!(!redirect_allowed(&u(bad), 0), "{bad}");
        }
    }

    #[test]
    fn release_body_is_capped_before_parsing() {
        let ok = br#"{"tag_name":"v9.9.9","html_url":"https://x","body":null}"#;
        assert_eq!(parse_release(ok).unwrap().tag_name, "v9.9.9");
        let mut big = br#"{"tag_name":"v1","html_url":"h","body":""#.to_vec();
        big.resize(MAX_RESPONSE_BYTES as usize + 1, b'a');
        assert!(parse_release(&big).unwrap_err().contains("too large"));
        assert!(parse_release(b"not json").is_err());
    }

    #[test]
    fn fake_latest_rejects_older() {
        // Force a clearly older version.
        let info = parse_fake_latest("0.0.1");
        assert!(info.is_none());
    }
}
