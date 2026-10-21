use std::{path::Path, sync::OnceLock, time::SystemTime};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    model::Quota,
    quota_cache::{http_client, read_bounded, round, Identity, KeyedCache, Unavailable},
    TokenSnapshot,
};

static CACHE: OnceLock<KeyedCache> = OnceLock::new();

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
    // Session logs may belong to another login or a different model's quota.
    // Only the authenticated account endpoint can supply current limits.
    let mut jobs: Vec<(Identity, Tokens)> = Vec::new();
    let mut identities: Vec<Option<Identity>> = Vec::new();
    for provider in snapshot.providers.iter_mut().filter(|p| p.id == "codex") {
        provider.quotas.clear();
        let Some(tokens) = credentials_in(&provider.dir) else {
            provider.warnings.push("quota_unavailable");
            identities.push(None);
            continue;
        };
        provider.auth_status = "credentials_present";
        let identity = identity_of(&tokens);
        jobs.push((identity, tokens));
        identities.push(Some(identity));
    }
    let cache = CACHE.get_or_init(Default::default);
    let results = round(cache, jobs, manual, fetch);
    let mut identities = identities.into_iter();
    for provider in snapshot.providers.iter_mut().filter(|p| p.id == "codex") {
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

/// sha256(account_id, 0, access_token): a new login or a refreshed token is a
/// new identity.
fn identity_of(tokens: &Tokens) -> Identity {
    let mut hash = Sha256::new();
    hash.update(tokens.account_id.as_bytes());
    hash.update([0]);
    hash.update(tokens.access_token.as_bytes());
    hash.finalize().into()
}

fn credentials_in(root: &Path) -> Option<Tokens> {
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
    let client = http_client()?;
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
