use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    model::Quota,
    quota_cache::{read_bounded, Cached, Unavailable},
    TokenSnapshot,
};

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

#[derive(Deserialize)]
struct Credentials {
    auth_mode: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: String,
}

#[derive(Deserialize)]
struct Usage {
    rate_limit: Option<Limits>,
}

#[derive(Deserialize)]
struct Limits {
    primary_window: Option<Window>,
    secondary_window: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    used_percent: f64,
    limit_window_seconds: u64,
    reset_at: Option<u64>,
}

pub(super) fn enrich(snapshot: &mut TokenSnapshot, manual: bool) {
    let Some(provider) = snapshot.providers.iter_mut().find(|p| p.id == "codex") else {
        return;
    };
    // Session logs may belong to another login or a different model's quota.
    // Only the authenticated account endpoint can supply current limits.
    provider.quotas.clear();
    let Some(tokens) = credentials() else {
        *CACHE.lock() = None;
        snapshot.warnings.push("codex_quota_unavailable".into());
        return;
    };
    provider.auth_status = "credentials_present";
    let mut hash = Sha256::new();
    hash.update(tokens.account_id.as_bytes());
    hash.update([0]);
    hash.update(tokens.access_token.as_bytes());
    let identity: [u8; 32] = hash.finalize().into();
    let mut cache = CACHE.lock();
    if cache
        .as_ref()
        .map_or(true, |c| c.needs_refresh(identity, manual))
    {
        let result = fetch(&tokens);
        *cache = Some(Cached::refreshed(cache.take(), identity, result));
    }
    if let Some(cached) = cache.as_ref() {
        provider.quotas = cached.quotas.clone();
        if cached.unavailable {
            snapshot.warnings.push(
                if cached.quotas.is_empty() {
                    "codex_quota_unavailable"
                } else {
                    "codex_quota_stale"
                }
                .into(),
            );
        }
    }
}

fn credentials() -> Option<Tokens> {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    let file = std::fs::File::open(root.join("auth.json")).ok()?;
    let bytes = read_bounded(file).ok()?;
    let credentials: Credentials = serde_json::from_slice(&bytes).ok()?;
    if credentials
        .auth_mode
        .as_deref()
        .is_some_and(|mode| mode != "chatgpt")
    {
        return None;
    }
    let tokens = credentials.tokens?;
    if tokens.access_token.trim().is_empty() || tokens.account_id.trim().is_empty() {
        return None;
    }
    Some(tokens)
}

fn fetch(tokens: &Tokens) -> Result<Vec<Quota>, Unavailable> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Unavailable)?;
    let response = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(&tokens.access_token)
        .header("ChatGPT-Account-Id", &tokens.account_id)
        .header("User-Agent", "codex-cli")
        .header("Cache-Control", "no-cache, no-store")
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| Unavailable)?;
    let bytes = read_bounded(response)?;
    let usage: Usage = serde_json::from_slice(&bytes).map_err(|_| Unavailable)?;
    let limits = usage.rate_limit.ok_or(Unavailable)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| Unavailable)?
        .as_secs();
    let mut quotas = Vec::new();
    for window in [limits.primary_window, limits.secondary_window]
        .into_iter()
        .flatten()
    {
        if !window.used_percent.is_finite()
            || !(0.0..=100.0).contains(&window.used_percent)
            || window.limit_window_seconds == 0
            || window.limit_window_seconds % 60 != 0
        {
            return Err(Unavailable);
        }
        quotas.push(Quota {
            window_minutes: window.limit_window_seconds / 60,
            used_percent: window.used_percent,
            resets_at: window.reset_at,
            observed_at: now,
        });
    }
    if quotas.is_empty() {
        return Err(Unavailable);
    }
    Ok(quotas)
}
