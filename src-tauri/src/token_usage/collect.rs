use super::{
    aggregate::{aggregate, FileView},
    disk::Budget,
    index::{Index, RootSpec},
    model::{ProviderUsage, TokenSnapshot, TokenTotals},
    retired::static_provider,
};
use crate::accounts::{account_dirs, distinct, AgentDirs};
use crate::agent_sessions::AgentKind;
use crate::config::model::AccountProfile;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
    time::{Duration, SystemTime},
};

/// What one refresh may spend indexing: reading stops at the first of these.
const TICK_BYTES: u64 = 256 * 1024 * 1024;
const TICK_TIME: Duration = Duration::from_millis(1500);

static INDEX: OnceLock<Mutex<Index>> = OnceLock::new();

/// Where the index shards live: `<config_dir>/ymux/token-index`.
pub(super) fn index_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("ymux").join("token-index"))
}

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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Lock `m`, taking the guard of a poisoned mutex too: one panic in a refresh
/// must not switch usage tracking off for the rest of the run (the index is
/// rebuilt from shards and transcripts, so a half-done refresh is harmless).
pub(super) fn lock_recovering<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn collect(accounts: &[AccountProfile]) -> TokenSnapshot {
    let dirs = account_dirs(
        accounts,
        |k| std::env::var(k).ok(),
        dirs::home_dir().as_deref(),
    );
    let roots = scan_roots(&dirs);
    let mut index = lock_recovering(INDEX.get_or_init(|| Mutex::new(Index::new(None))));
    if let Some(dir) = index_dir() {
        index.set_dir_if_unset(dir);
    }
    collect_with(
        &mut index,
        &roots,
        now_secs(),
        Budget::new(TICK_BYTES, Some(TICK_TIME)),
    )
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

/// One refresh of a throw-away, memory-only index with a byte budget but no
/// time limit. It does not share the process-wide index: that one keeps the
/// history of roots it stops seeing, which a one-off scan of other roots must
/// neither inherit nor leave behind.
pub fn collect_from_roots(roots: &[ScanRoot], now: u64) -> TokenSnapshot {
    collect_with(&mut Index::new(None), roots, now, Budget::bytes(TICK_BYTES))
}

/// What the index lists for each root: the folders under it that hold JSONL.
pub(super) fn specs_of(roots: &[ScanRoot]) -> Vec<RootSpec> {
    roots
        .iter()
        .map(|scan| RootSpec {
            provider: scan.provider,
            owner: scan.owner.clone(),
            dir: scan.dir.clone(),
            folders: if scan.provider == "claude" {
                vec![scan.dir.join("projects")]
            } else {
                vec![
                    scan.dir.join("sessions"),
                    scan.dir.join("archived_sessions"),
                ]
            },
        })
        .collect()
}

/// Refresh `index` for `roots` within `budget`, then build the snapshot from
/// what is indexed. Roots must list the default account first (see
/// [`aggregate`]'s dedup).
pub(super) fn collect_with(
    index: &mut Index,
    roots: &[ScanRoot],
    now: u64,
    mut budget: Budget,
) -> TokenSnapshot {
    let mut snapshot = TokenSnapshot {
        collected_at: now,
        providers: Vec::new(),
        rows: Vec::new(),
        hourly: Vec::new(),
        warnings: Vec::new(),
        indexing: false,
        indexed_bytes: 0,
        total_bytes: 0,
        unreadable_files: 0,
        skipped_entries: 0,
        file_limit_hit: false,
        listing_incomplete: false,
        retired_unavailable: false,
        retired_unavailable_reason: String::new(),
    };
    let specs = specs_of(roots);
    let progress = index.refresh(&specs, &mut budget, now);
    snapshot.indexed_bytes = progress.indexed;
    snapshot.total_bytes = progress.total;
    snapshot.indexing = progress.working;
    let health = index.health(&specs);
    snapshot.unreadable_files = health.unreadable;
    snapshot.skipped_entries = health.skipped;
    snapshot.file_limit_hit = health.file_limit_hit;
    snapshot.listing_incomplete = health.listing_incomplete;
    snapshot.retired_unavailable = health.retired_unavailable;
    snapshot.retired_unavailable_reason = health.retired_reason;

    let mut accounts: Vec<String> = Vec::new();
    let mut views = Vec::new();
    for scan in roots {
        let id = scan.provider;
        let auth = if id == "claude" {
            ".credentials.json"
        } else {
            "auth.json"
        };
        let account = accounts
            .iter()
            .position(|a| *a == scan.owner)
            .unwrap_or_else(|| {
                accounts.push(scan.owner.clone());
                accounts.len() - 1
            });
        let (problems, truncated) = index.root_problems(id, &scan.dir);
        let mut provider = ProviderUsage {
            id,
            account: scan.owner.clone(),
            also: scan.also.clone(),
            auth_status: if scan.dir.join(auth).is_file() {
                "credentials_present"
            } else {
                "not_detected"
            },
            five_hour: TokenTotals::default(),
            seven_day: TokenTotals::default(),
            all_time: TokenTotals::default(),
            quotas: Vec::new(),
            files: 0,
            skipped: problems,
            warnings: Vec::new(),
            dir: scan.dir.clone(),
            claude_env: scan.claude_env.clone(),
        };
        if truncated {
            provider.warnings.push("file_limit");
        }
        for file in index.root_files(id, &scan.dir) {
            provider.files += 1;
            provider.skipped += file.skipped;
            let latest = file
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
                provider.quotas = file
                    .quotas
                    .iter()
                    .filter(|q| Some(q.observed_at) == latest)
                    .cloned()
                    .collect();
            }
            views.push(FileView {
                account,
                provider: id,
                file,
            });
        }
        if provider.skipped > 0 {
            provider.warnings.push("partial");
        }
        snapshot.providers.push(provider);
    }
    // Files of a root nobody lists any more that still exist: still counted,
    // after every configured root so those win the dedup.
    for file in index.unconfigured_files() {
        let Some(id) = static_provider(&file.provider) else {
            continue;
        };
        let account = accounts
            .iter()
            .position(|a| *a == file.owner)
            .unwrap_or_else(|| {
                accounts.push(file.owner.clone());
                accounts.len() - 1
            });
        views.push(FileView {
            account,
            provider: id,
            file,
        });
    }
    aggregate(&mut snapshot, &views, &accounts, index.retired());
    snapshot
}

#[cfg(test)]
pub(super) fn now_for_tests() -> u64 {
    now_secs()
}
