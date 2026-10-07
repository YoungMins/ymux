use super::{
    aggregate::aggregate,
    model::{ProviderUsage, TokenSnapshot, TokenTotals},
    parse::Parsed,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

const MAX_FILES: usize = 20000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
struct Cached {
    len: u64,
    modified: Option<SystemTime>,
    parsed: Parsed,
}
static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Cached>>> = OnceLock::new();

pub fn collect() -> TokenSnapshot {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let home = dirs::home_dir().unwrap_or_default();
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    collect_from(&claude, &codex, now)
}

pub fn collect_from(claude: &Path, codex: &Path, now: u64) -> TokenSnapshot {
    let mut snapshot = TokenSnapshot {
        collected_at: now,
        providers: Vec::new(),
        rows: Vec::new(),
        warnings: Vec::new(),
    };
    let mut events = Vec::new();
    let mut seen = BTreeSet::new();
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(BTreeMap::new())).lock() else {
        snapshot.warnings.push("collector_unavailable".into());
        return snapshot;
    };
    for (id, root, folders, auth) in [
        ("claude", claude, &["projects"][..], ".credentials.json"),
        (
            "codex",
            codex,
            &["sessions", "archived_sessions"][..],
            "auth.json",
        ),
    ] {
        let mut budget = MAX_SCAN_BYTES;
        let mut event_budget = 25_000;
        let mut provider = ProviderUsage {
            id,
            auth_status: if root.join(auth).is_file() {
                "credentials_present"
            } else {
                "not_detected"
            },
            five_hour: TokenTotals::default(),
            seven_day: TokenTotals::default(),
            all_time: TokenTotals::default(),
            quotas: Vec::new(),
            files: 0,
            skipped: 0,
        };
        let mut paths = Vec::new();
        for folder in folders {
            super::disk::walk(&root.join(folder), &mut paths, &mut provider.skipped, 0);
        }
        if paths.len() >= MAX_FILES {
            snapshot.warnings.push(format!("{id}_file_limit"));
        }
        paths.sort_by_cached_key(|path| {
            std::cmp::Reverse(fs::metadata(path).ok().and_then(|m| m.modified().ok()))
        });
        for path in paths {
            let Ok(meta) = fs::metadata(&path) else {
                provider.skipped += 1;
                continue;
            };
            if meta.len() > MAX_FILE_BYTES || meta.len() > budget {
                provider.skipped += 1;
                continue;
            }
            budget -= meta.len();
            seen.insert(path.clone());
            let modified = meta.modified().ok();
            let unchanged = cache
                .get(&path)
                .is_some_and(|c| c.len == meta.len() && c.modified == modified);
            if !unchanged {
                let fallback = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                match super::disk::parse_file(&path, id, fallback, meta.len()) {
                    Ok(parsed) => {
                        cache.insert(
                            path.clone(),
                            Cached {
                                len: meta.len(),
                                modified,
                                parsed,
                            },
                        );
                    }
                    Err(_) => {
                        provider.skipped += 1;
                        cache.remove(&path);
                        continue;
                    }
                }
            }
            let Some(cached) = cache.get(&path) else {
                continue;
            };
            if cached.parsed.events.len() > event_budget {
                provider.skipped += 1;
                seen.remove(&path);
                cache.remove(&path);
                continue;
            }
            event_budget -= cached.parsed.events.len();
            provider.files += 1;
            provider.skipped += cached.parsed.skipped;
            for event in &cached.parsed.events {
                events.push(super::model::Event {
                    provider: event.provider,
                    session: event.session.clone(),
                    id: event.id.clone(),
                    timestamp: event.timestamp,
                    model: event.model.clone(),
                    project: event.project.clone(),
                    tokens: event.tokens,
                });
            }
            let latest = cached
                .parsed
                .quotas
                .iter()
                .filter(|q| q.observed_at <= now)
                .map(|q| q.observed_at)
                .max();
            if latest.is_some_and(|time| {
                provider
                    .quotas
                    .first()
                    .map_or(true, |p| time > p.observed_at)
            }) {
                provider.quotas = cached
                    .parsed
                    .quotas
                    .iter()
                    .filter(|q| Some(q.observed_at) == latest)
                    .cloned()
                    .collect();
            }
        }
        if provider.skipped > 0 {
            snapshot.warnings.push(format!("{id}_partial"));
        }
        snapshot.providers.push(provider);
    }
    cache.retain(|path, _| seen.contains(path));
    aggregate(&mut snapshot, events);
    snapshot
}
