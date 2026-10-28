use super::{
    aggregate::aggregate,
    model::{ProviderUsage, TokenSnapshot, TokenTotals},
    parse::Parsed,
};
use crate::accounts::{account_dirs, distinct, AgentDirs};
use crate::agent_sessions::AgentKind;
use crate::config::model::AccountProfile;
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
    /// Set when the file had more events than the budget left for it: the
    /// events are dropped (not kept in memory) and this is how many there
    /// were, so an unchanged file is skipped again without re-reading it.
    over_budget: Option<usize>,
}
static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Cached>>> = OnceLock::new();

/// One directory to scan for one provider, and who owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRoot {
    pub provider: &'static str,
    /// The first account that lists the directory (`""` = default).
    pub owner: String,
    /// Later, different accounts that use the same directory.
    pub also: Vec<String>,
    pub dir: PathBuf,
    /// The owner's literal `CLAUDE_CONFIG_DIR` (Claude roots only).
    pub claude_env: Option<String>,
}

/// Every provider's distinct directories across `dirs` (default account
/// first, so it owns a directory it shares with a profile).
pub fn scan_roots(dirs: &[(String, AgentDirs)]) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    for (provider, agent) in [("claude", AgentKind::Claude), ("codex", AgentKind::Codex)] {
        let pairs = dirs
            .iter()
            .map(|(owner, d)| (owner.clone(), d.for_agent(agent).to_owned()));
        for group in distinct(pairs) {
            let claude_env = dirs
                .iter()
                .find(|(owner, _)| *owner == group.owner)
                .and_then(|(_, d)| d.claude_env.clone())
                .filter(|_| provider == "claude");
            roots.push(ScanRoot {
                provider,
                owner: group.owner,
                also: group.also,
                dir: PathBuf::from(group.dir),
                claude_env,
            });
        }
    }
    roots
}

pub fn collect(accounts: &[AccountProfile]) -> TokenSnapshot {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dirs = account_dirs(
        accounts,
        |k| std::env::var(k).ok(),
        dirs::home_dir().as_deref(),
    );
    collect_from_roots(&scan_roots(&dirs), now)
}

/// Single-account form: the two directories are the default account's.
pub fn collect_from(claude: &Path, codex: &Path, now: u64) -> TokenSnapshot {
    let root = |provider, dir: &Path| ScanRoot {
        provider,
        owner: String::new(),
        also: Vec::new(),
        dir: dir.to_path_buf(),
        claude_env: None,
    };
    collect_from_roots(&[root("claude", claude), root("codex", codex)], now)
}

/// One pass over every root with one cache lock, one `seen` set and one
/// eviction: scanning per account would evict each other's cache entries and
/// re-parse everything on every refresh. Roots must list the default account
/// first (see [`aggregate`]'s dedup).
pub fn collect_from_roots(roots: &[ScanRoot], now: u64) -> TokenSnapshot {
    let mut snapshot = TokenSnapshot {
        collected_at: now,
        providers: Vec::new(),
        rows: Vec::new(),
        hourly: Vec::new(),
        warnings: Vec::new(),
    };
    let mut events = Vec::new();
    let mut accounts: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(BTreeMap::new())).lock() else {
        snapshot.warnings.push("collector_unavailable".into());
        return snapshot;
    };
    for scan in roots {
        let id = scan.provider;
        let root = scan.dir.as_path();
        let (folders, auth) = if id == "claude" {
            (&["projects"][..], ".credentials.json")
        } else {
            (&["sessions", "archived_sessions"][..], "auth.json")
        };
        let account = accounts
            .iter()
            .position(|a| *a == scan.owner)
            .unwrap_or_else(|| {
                accounts.push(scan.owner.clone());
                accounts.len() - 1
            });
        let mut budget = MAX_SCAN_BYTES;
        let mut event_budget = 25_000;
        let mut provider = ProviderUsage {
            id,
            account: scan.owner.clone(),
            also: scan.also.clone(),
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
            warnings: Vec::new(),
            dir: scan.dir.clone(),
            claude_env: scan.claude_env.clone(),
        };
        let mut paths = Vec::new();
        for folder in folders {
            super::disk::walk(&root.join(folder), &mut paths, &mut provider.skipped, 0);
        }
        if paths.len() >= MAX_FILES {
            provider.warnings.push("file_limit");
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
            let over = cache.get(&path).and_then(|c| c.over_budget);
            if unchanged && over.is_some_and(|n| n > event_budget) {
                provider.skipped += 1;
                continue;
            }
            if !unchanged || over.is_some() {
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
                                over_budget: None,
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
                let n = cached.parsed.events.len();
                cache.insert(
                    path.clone(),
                    Cached {
                        len: meta.len(),
                        modified,
                        parsed: Parsed::default(),
                        over_budget: Some(n),
                    },
                );
                continue;
            }
            event_budget -= cached.parsed.events.len();
            provider.files += 1;
            provider.skipped += cached.parsed.skipped;
            for event in &cached.parsed.events {
                events.push(super::model::Event {
                    account,
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
            provider.warnings.push("partial");
        }
        snapshot.providers.push(provider);
    }
    cache.retain(|path, _| seen.contains(path));
    aggregate(&mut snapshot, events, &accounts);
    snapshot
}
