//! The persistent per-transcript index behind the usage view.
//!
//! Every transcript gets a compact [`FileSummary`] keyed by `(path, len,
//! mtime)`: its deduplicated events (enough to rebuild every row, the heatmap
//! hours and the rolling windows), its quota samples and the parser context
//! needed to carry on reading. A file whose `(len, mtime)` has not changed is
//! never read again; one that only grew is parsed from the saved byte offset;
//! anything else is parsed from scratch. First-time indexing of a large
//! history is spread over several refreshes by a work [`Budget`], newest file
//! first.
//!
//! Cross-file dedup (a Claude message copied into a fork, an archived Codex
//! session) cannot be decided per file, so the summaries keep the data it
//! needs (one entry per message key, one checkpoint per Codex total) and
//! `aggregate` applies it over the union. Within a file the dedup is exact and
//! already applied, which keeps a summary near the size of its distinct
//! messages, not of its lines.
//!
//! Each summary persists as its own small JSON shard
//! (`<dir>/<hash of path>.json`, written atomically), so a growing session
//! rewrites one shard, not the whole history.
//!
//! A shard is a cache **only while its transcript exists**: one that fails to
//! parse, has another [`INDEX_VERSION`], names another file or is internally
//! inconsistent is then ignored and the file is simply parsed again
//! ([`Index::ensure_loaded`]). Once the transcript is gone the shard is the
//! only copy of that history, so it is read with [`decode_shard`] instead:
//! an older version goes through [`migrate_shard`], and a shard that still
//! cannot be used is kept aside as `*.broken-<time>`, never deleted.
//!
//! A file that disappears (deleted, or its whole root missing) is *retired*
//! into the [`Retired`] store, see `retired.rs`, and only then. The rules:
//!
//! * One way out of the figures: a file leaves `files`/`stats` only through
//!   retirement, and retirement needs (a) the file confirmed gone by a probe
//!   of THAT file (`disk::presence`: not-found while its root is reachable)
//!   and (b) a usable store. A truncated or failed listing, an unavailable
//!   root, an unusable store: the entry stays untouched and is retried.
//! * A retired file's shard is deleted two refreshes later, after the store
//!   was read back from disk and found to cover it (see `retired.rs`).
//! * A complete summary is not replaced on disk by an unfinished re-parse
//!   (`superseded`), and if the file vanishes meanwhile both are retired.
use super::{
    disk::{self, Budget, Listing, Presence, Stat, MAX_FILES},
    model::{Event, Quota, TokenTotals},
    parse::Parsed,
    retired::{
        decode_retired, migrate_retired, Candidate, Retired, StoreError, RETIRED_BACKUP,
        RETIRED_FILE, RETIRED_FORMAT,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};

/// Bump when the shard layout or what a summary means changes.
pub(super) const INDEX_VERSION: u32 = 1;
/// The advisory lock around writes of the retired store.
const RETIRED_LOCK: &str = "retired.lock";
/// A lock this old belongs to a process that died (a refresh takes seconds).
const LOCK_STALE: Duration = Duration::from_secs(5 * 60);
/// Quota samples kept per file: the newest few distinct observation times.
const QUOTA_GROUPS: usize = 8;
/// A file written within this long is re-stat-ed on every refresh; older ones
/// only on a full sweep.
const HOT_SECS: u64 = 6 * 3600;

/// One distinct event of a transcript, in the shard's compact spelling.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct StoredEvent {
    /// Claude: hash of the message id (the cross-file dedup key). Codex: 0.
    pub k: u64,
    /// Seconds since the epoch.
    pub at: u64,
    pub i: u64,
    pub o: u64,
    pub r: u64,
    pub w: u64,
    pub t: u64,
    /// Indices into the summary's `models`, `projects` and `sessions`.
    pub m: u32,
    pub p: u32,
    pub n: u32,
}

impl StoredEvent {
    pub fn tokens(&self) -> TokenTotals {
        TokenTotals {
            input: self.i,
            output: self.o,
            cache_read: self.r,
            cache_write: self.w,
            total: self.t,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct FileSummary {
    pub version: u32,
    pub path: String,
    pub provider: String,
    /// The account whose root listed the file last (`""` = default); a retired
    /// file keeps the attribution it had.
    pub owner: String,
    /// The `(len, mtime)` the last pass was started against.
    pub len: u64,
    pub mtime: Option<u64>,
    /// Start of the first line not consumed (after a half-written last line).
    pub offset: u64,
    /// How far the last pass read; below `len` while the budget ran out.
    pub scanned: u64,
    /// Hash of the consumed prefix, see [`disk::fingerprint`].
    pub fingerprint: u64,
    pub skipped: usize,
    /// The parser's running session / project / model at `offset`.
    pub session: String,
    pub project: String,
    pub model: String,
    pub models: Vec<String>,
    pub projects: Vec<String>,
    pub sessions: Vec<String>,
    pub events: Vec<StoredEvent>,
    pub quotas: Vec<Quota>,
}

fn intern(list: &mut Vec<String>, value: &str) -> u32 {
    let at = list.iter().position(|v| v == value).unwrap_or_else(|| {
        list.push(value.to_owned());
        list.len() - 1
    });
    u32::try_from(at).unwrap_or(u32::MAX)
}

impl FileSummary {
    fn new(path: &Path, provider: &str) -> Self {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        Self {
            version: INDEX_VERSION,
            path: path.to_string_lossy().into_owned(),
            provider: provider.to_owned(),
            session: stem.to_owned(),
            project: "unknown".into(),
            model: "unknown".into(),
            ..Self::default()
        }
    }

    pub fn is_complete(&self) -> bool {
        self.scanned == self.len
    }

    pub fn is_claude(&self) -> bool {
        self.provider == "claude"
    }

    /// Fold one parsed event in. Claude: one entry per message key, the larger
    /// record wins (streamed updates repeat an id with growing output); the
    /// first one wins a tie. Codex: checkpoints are kept, except a repeat of
    /// the previous one.
    fn absorb(&mut self, event: Event, by_key: &mut HashMap<u64, usize>) {
        let claude = self.is_claude();
        let key = if claude {
            disk::fnv1a(event.id.as_bytes())
        } else {
            0
        };
        let stored = StoredEvent {
            k: key,
            at: event.timestamp,
            i: event.tokens.input,
            o: event.tokens.output,
            r: event.tokens.cache_read,
            w: event.tokens.cache_write,
            t: event.tokens.total,
            m: intern(&mut self.models, &event.model),
            p: intern(&mut self.projects, &event.project),
            n: if claude {
                0
            } else {
                intern(&mut self.sessions, &event.session)
            },
        };
        if claude {
            match by_key.get(&key) {
                Some(&at) => {
                    if stored.t > self.events[at].t {
                        self.events[at] = stored;
                    }
                }
                None => {
                    by_key.insert(key, self.events.len());
                    self.events.push(stored);
                }
            }
        } else if !self
            .events
            .last()
            .is_some_and(|last| last.n == stored.n && last.t == stored.t && last.at <= stored.at)
        {
            self.events.push(stored);
        }
    }

    fn prune_quotas(&mut self) {
        self.quotas.sort_by_key(|q| Reverse(q.observed_at));
        let mut groups = 0;
        let mut last = None;
        self.quotas.retain(|q| {
            if last != Some(q.observed_at) {
                last = Some(q.observed_at);
                groups += 1;
            }
            groups <= QUOTA_GROUPS
        });
    }

    /// Cannot make the aggregation index out of range.
    pub fn consistent(&self) -> bool {
        let codex = self.provider == "codex";
        (codex || self.is_claude())
            && self.offset <= self.scanned
            && self.scanned <= self.len
            && self.events.iter().all(|e| {
                (e.m as usize) < self.models.len()
                    && (e.p as usize) < self.projects.len()
                    && (!codex || (e.n as usize) < self.sessions.len())
            })
    }

    /// A shard of a file that still exists is trusted only if it is current,
    /// about this file and consistent.
    pub fn valid_for(&self, path: &Path, provider: &str) -> bool {
        self.version == INDEX_VERSION
            && self.path == path.to_string_lossy()
            && self.provider == provider
            && self.consistent()
    }
}

/// Upgrade a shard of an older [`INDEX_VERSION`] to the current layout.
///
/// Nothing older exists yet. When `INDEX_VERSION` is bumped, add the arm that
/// reads the old layout (`from` is its number, `shard` the parsed JSON) and
/// returns the summary in the current layout, `version` included. It is used
/// only for shards whose transcript is gone (their history exists nowhere
/// else); a shard of a transcript that still exists is simply re-parsed.
pub(super) fn migrate_shard(_from: u32, _shard: serde_json::Value) -> Option<FileSummary> {
    None
}

/// What a shard whose transcript is gone turned out to hold.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // transient, one per shard read
pub(super) enum Decoded {
    Summary(FileSummary),
    /// Written by a newer yMux: leave it alone.
    Newer(u32),
    Bad(String),
}

/// Read a shard that must not be lost. `current` and `migrate` are parameters
/// so tests can play a version bump; the app passes [`INDEX_VERSION`] and
/// [`migrate_shard`].
pub(super) fn decode_shard(
    bytes: &[u8],
    current: u32,
    migrate: &dyn Fn(u32, serde_json::Value) -> Option<FileSummary>,
) -> Decoded {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        version: u32,
    }
    let bad = |e: serde_json::Error| Decoded::Bad(e.to_string());
    let probe: Probe = match serde_json::from_slice(bytes) {
        Ok(p) => p,
        Err(e) => return bad(e),
    };
    if probe.version > current {
        return Decoded::Newer(probe.version);
    }
    let summary = if probe.version == current {
        match serde_json::from_slice::<FileSummary>(bytes) {
            Ok(s) => s,
            Err(e) => return bad(e),
        }
    } else {
        let value = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(e) => return bad(e),
        };
        match migrate(probe.version, value) {
            Some(s) if s.version == current => s,
            _ => return Decoded::Bad(format!("no migration from version {}", probe.version)),
        }
    };
    if summary.consistent() {
        Decoded::Summary(summary)
    } else {
        Decoded::Bad("inconsistent summary".into())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Rename `path` to `<name>.<label>-<now>` (like config.toml's
/// `.broken-<now>`), keeping the bytes. `None` when that failed.
fn set_aside(path: &Path, label: &str) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".{label}-{}", now_unix()));
    let aside = path.with_file_name(name);
    fs::rename(path, &aside).ok()?;
    Some(aside)
}

/// Names one holder of the lock, so a lock is only ever released by the one
/// who took it (two `Index`es of one process included).
fn new_lock_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// A file's length and mtime, the cheap look that says whether it may have
/// changed (`None` when it cannot be statted: then it is read).
fn stamp_of(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

fn is_shard_name(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "json")
        && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| {
            s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

pub(super) fn shard_path(dir: &Path, path: &Path) -> PathBuf {
    dir.join(format!(
        "{:016x}.json",
        disk::fnv1a(path.to_string_lossy().as_bytes())
    ))
}

/// One directory to index: where it lives and which sub-folders hold JSONL.
pub(super) struct RootSpec {
    pub provider: &'static str,
    /// The account that owns the directory (`""` = default).
    pub owner: String,
    pub dir: PathBuf,
    pub folders: Vec<PathBuf>,
}

#[derive(Default)]
struct RootState {
    stats: BTreeMap<PathBuf, Stat>,
    last_sweep: Option<Instant>,
    walk_skipped: usize,
    truncated: bool,
    incomplete: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Progress {
    /// Bytes read into summaries, and bytes in every listed file (unreadable
    /// ones included).
    pub indexed: u64,
    pub total: u64,
    /// Some readable file is not fully indexed yet. Unreadable files do not
    /// count: they would keep this true forever.
    pub working: bool,
}

/// What a snapshot reports about the trustworthiness of its figures.
#[derive(Clone, Debug, Default)]
pub(super) struct Health {
    pub unreadable: usize,
    pub skipped: usize,
    pub file_limit_hit: bool,
    pub listing_incomplete: bool,
    pub retired_unavailable: bool,
    pub retired_reason: String,
}

/// A summary whose file is gone, waiting to be folded into the retired store.
#[derive(Clone)]
struct Pending {
    /// Where aggregation listed it (spec index); `usize::MAX` when no
    /// configured root holds it.
    order: usize,
    summary: FileSummary,
    /// Its shard, deleted two refreshes after the retired store is on disk.
    shard: Option<PathBuf>,
}

pub(super) type Lister = Box<dyn Fn(&RootSpec, usize) -> Listing + Send>;
pub(super) type Probe = Box<dyn Fn(&Path, &Path) -> Presence + Send>;
type Migrator = fn(u32, serde_json::Value) -> Option<FileSummary>;

enum RetiredState {
    Unloaded,
    Ready,
    /// The store on disk cannot be used. It is left exactly as it is, nothing
    /// is retired or deleted, and `retry` says whether reading it again can
    /// help (a transient I/O error) or not (corrupt, newer format).
    Unavailable {
        reason: String,
        retry: bool,
    },
}

/// What happened to a shard of a file nothing lists.
#[allow(clippy::large_enum_variant)] // transient, one per shard read
enum Gone {
    Summary(FileSummary),
    /// Written by a newer yMux or set aside: nothing more to do.
    Left,
    /// Could not be set aside: try the whole pass again.
    Failed,
}

pub(super) struct Index {
    dir: Option<PathBuf>,
    /// How often a root's whole tree is listed again; in between only recently
    /// written files are re-stat-ed.
    pub sweep_every: Duration,
    /// How often changed summaries are written back.
    pub persist_every: Duration,
    /// How often a retired store that changed only by upgrades or compaction
    /// is written (a retirement is written at once).
    pub retired_persist_every: Duration,
    /// Files per listing before it stops (see `disk::MAX_FILES`).
    pub file_limit: usize,
    lister: Lister,
    probe: Probe,
    shard_version: u32,
    migrator: Migrator,
    files: HashMap<PathBuf, FileSummary>,
    /// The last complete summary of a file whose re-parse has not finished.
    superseded: HashMap<PathBuf, FileSummary>,
    roots: HashMap<(&'static str, PathBuf), RootState>,
    /// The roots the latest refresh was asked about.
    wanted: HashSet<(&'static str, PathBuf)>,
    dirty: HashSet<PathBuf>,
    unreadable: HashSet<PathBuf>,
    last_persist: Option<Instant>,
    gc_done: bool,
    was_indexing: bool,
    read_bytes: u64,
    refreshes: u64,
    shard_write_failures: usize,
    retired: Retired,
    retired_state: RetiredState,
    warned_unavailable: bool,
    retired_dirty: bool,
    retired_urgent: bool,
    last_retired_write: Option<Instant>,
    last_deletion_try: Option<Instant>,
    /// Refresh number of the latest successful write of the retired store.
    committed_at: Option<u64>,
    pending: Vec<Pending>,
    /// Folded into the in-memory store but not yet on disk. If another process
    /// wrote the store first, ours is dropped for theirs and these are
    /// retired again against it (retiring is idempotent).
    unsaved: Vec<Pending>,
    /// Hash of the bytes of `retired.json` this process last loaded or wrote:
    /// a commit over a different file would erase another process's work.
    retired_seen: Option<u64>,
    /// Length and mtime of `retired.json` when `retired_seen` was taken: the
    /// per-refresh look reads and hashes the file only when they differ (the
    /// file can be tens of MB). A commit still compares the real content.
    retired_stamp: Option<(u64, Option<SystemTime>)>,
    /// Full reads of `retired.json` by the per-refresh look (tests).
    #[cfg(test)]
    pub sync_reads: usize,
    /// The store was replaced by the one on disk: settle again.
    resettle: bool,
    /// Whether to take the advisory lock around writes of the store. Always on
    /// in the app; tests turn it off to play a process that races without it.
    pub use_lock: bool,
    /// A lock older than this is left over from a process that died.
    pub lock_stale: Duration,
    lock_token: String,
    lock_held: bool,
    /// This refresh may write the retired store (usable, and locked).
    locked: bool,
    /// Test hook: do not look at the store on disk before a refresh.
    #[cfg(test)]
    pub skip_sync: bool,
    /// Test hook: stop persisting after this step, as a killed process would.
    #[cfg(test)]
    pub crash_after: Option<&'static str>,
    #[cfg(test)]
    crashed: bool,
}

impl Drop for Index {
    fn drop(&mut self) {
        self.unlock();
    }
}

impl Index {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            sweep_every: Duration::from_secs(60),
            persist_every: Duration::from_secs(10),
            retired_persist_every: Duration::from_secs(30),
            file_limit: MAX_FILES,
            lister: Box::new(|spec, limit| disk::list_folders(&spec.folders, limit)),
            probe: Box::new(disk::presence),
            shard_version: INDEX_VERSION,
            migrator: migrate_shard,
            files: HashMap::new(),
            superseded: HashMap::new(),
            roots: HashMap::new(),
            wanted: HashSet::new(),
            dirty: HashSet::new(),
            unreadable: HashSet::new(),
            last_persist: None,
            gc_done: false,
            was_indexing: false,
            read_bytes: 0,
            refreshes: 0,
            shard_write_failures: 0,
            retired: Retired::default(),
            retired_state: RetiredState::Unloaded,
            warned_unavailable: false,
            retired_dirty: false,
            retired_urgent: false,
            last_retired_write: None,
            last_deletion_try: None,
            committed_at: None,
            pending: Vec::new(),
            unsaved: Vec::new(),
            retired_seen: None,
            retired_stamp: None,
            #[cfg(test)]
            sync_reads: 0,
            resettle: false,
            use_lock: true,
            lock_stale: LOCK_STALE,
            lock_token: new_lock_token(),
            lock_held: false,
            locked: false,
            #[cfg(test)]
            skip_sync: false,
            #[cfg(test)]
            crash_after: None,
            #[cfg(test)]
            crashed: false,
        }
    }

    /// Start persisting under `dir` if nothing is configured yet.
    pub fn set_dir_if_unset(&mut self, dir: PathBuf) {
        if self.dir.is_none() {
            self.dir = Some(dir);
        }
    }

    /// Replace how roots are listed (tests: a listing that omits files or
    /// reports itself incomplete).
    #[cfg(test)]
    pub fn set_lister(&mut self, lister: Lister) {
        self.lister = lister;
    }

    /// Replace the "is this file really gone" probe (tests).
    #[cfg(test)]
    pub fn set_probe(&mut self, probe: Probe) {
        self.probe = probe;
    }

    /// Play another shard version (tests): `current` and a migrator.
    #[cfg(test)]
    pub fn set_shard_version(&mut self, current: u32, migrator: Migrator) {
        self.shard_version = current;
        self.migrator = migrator;
    }

    /// The history of files that are gone.
    pub fn retired(&self) -> &Retired {
        &self.retired
    }

    /// Live files held and events in them.
    #[cfg(test)]
    pub fn live_totals(&self) -> (usize, usize) {
        (
            self.files.len(),
            self.files.values().map(|s| s.events.len()).sum(),
        )
    }

    /// Bytes read from transcripts by the latest refresh.
    #[cfg(test)]
    pub fn last_read_bytes(&self) -> u64 {
        self.read_bytes
    }

    #[cfg(test)]
    pub fn summary(&self, path: &Path) -> Option<&FileSummary> {
        self.files.get(path)
    }

    /// `(len, mtime)` each held summary was made against (tests).
    #[cfg(test)]
    pub fn summary_stats(&self) -> HashMap<PathBuf, (u64, Option<u64>)> {
        self.files
            .iter()
            .map(|(p, s)| (p.clone(), (s.len, s.mtime)))
            .collect()
    }

    /// Lines the parser could not read, over all held summaries (tests).
    #[cfg(test)]
    pub fn skipped_lines(&self) -> usize {
        self.files.values().map(|s| s.skipped).sum()
    }

    /// Files with lines the parser could not read, and how many (tests).
    #[cfg(test)]
    pub fn skipped_by_file(&self) -> Vec<(PathBuf, usize)> {
        let mut files: Vec<_> = self
            .files
            .iter()
            .filter(|(_, s)| s.skipped > 0)
            .map(|(p, s)| (p.clone(), s.skipped))
            .collect();
        files.sort();
        files
    }

    /// Whether a complete summary of a re-parsed file is being kept (tests).
    #[cfg(test)]
    pub fn superseded_count(&self) -> usize {
        self.superseded.len()
    }

    #[cfg(test)]
    /// Shard writes that failed and are still owed (a complete shard kept on
    /// disk on purpose, see `superseded`, is not owed).
    pub fn shard_writes_owed(&self) -> usize {
        self.dirty
            .iter()
            .filter(|p| !self.superseded.contains_key(*p))
            .count()
    }

    /// Cumulative number of failed shard writes.
    #[cfg(test)]
    pub fn shard_write_failures(&self) -> usize {
        self.shard_write_failures
    }

    /// Write whatever is owed right now, ignoring the throttles.
    #[cfg(test)]
    pub fn flush(&mut self) {
        self.persist(true);
        self.lock();
        self.persist_retired();
        self.unlock();
    }

    /// The indexed files of one root as of the latest refresh, in path order.
    pub fn root_files<'a>(
        &'a self,
        provider: &'static str,
        dir: &Path,
    ) -> impl Iterator<Item = &'a FileSummary> + 'a {
        self.roots
            .get(&(provider, dir.to_path_buf()))
            .into_iter()
            .flat_map(|state| state.stats.keys())
            .filter_map(|path| self.current(path))
    }

    /// The summary of a file that counts: the last complete one while a
    /// from-scratch re-parse of it is unfinished (never both: the figures
    /// would otherwise dip until the new pass catches up, and a retirement in
    /// that window would fold the smaller value), else the file's own.
    fn current(&self, path: &Path) -> Option<&FileSummary> {
        self.superseded.get(path).or_else(|| self.files.get(path))
    }

    /// Files that are still indexed but belong to no root of the latest
    /// `refresh` (an account that was removed while its folder still exists).
    /// They keep counting; each is re-probed at the sweep cadence and retired
    /// only when confirmed gone.
    pub fn unconfigured_files(&self) -> impl Iterator<Item = &FileSummary> + '_ {
        self.roots
            .iter()
            .filter(|(key, _)| !self.wanted.contains(*key))
            .flat_map(|(_, state)| state.stats.keys())
            .filter_map(|path| self.current(path))
    }

    /// What a root's card counts as skipped (unreadable files and listing
    /// problems) and whether its file list was cut at the file limit.
    pub fn root_problems(&self, provider: &'static str, dir: &Path) -> (usize, bool) {
        let Some(state) = self.roots.get(&(provider, dir.to_path_buf())) else {
            return (0, false);
        };
        let unreadable = state
            .stats
            .keys()
            .filter(|p| self.unreadable.contains(*p))
            .count();
        (state.walk_skipped + unreadable, state.truncated)
    }

    /// How far the figures can be trusted, over `specs`' roots.
    pub fn health(&self, specs: &[RootSpec]) -> Health {
        let mut health = Health::default();
        for spec in specs {
            let Some(state) = self.roots.get(&(spec.provider, spec.dir.clone())) else {
                continue;
            };
            health.unreadable += state
                .stats
                .keys()
                .filter(|p| self.unreadable.contains(*p))
                .count();
            health.skipped += state.walk_skipped;
            health.file_limit_hit |= state.truncated;
            health.listing_incomplete |= state.incomplete;
        }
        if let RetiredState::Unavailable { reason, .. } = &self.retired_state {
            health.retired_unavailable = true;
            health.retired_reason.clone_from(reason);
        }
        health
    }

    fn retired_usable(&self) -> bool {
        matches!(self.retired_state, RetiredState::Ready)
    }

    /// The store may be changed by this process right now.
    fn can_write(&self) -> bool {
        self.locked && self.retired_usable()
    }

    /// Take the advisory lock that lets one process at a time retire files and
    /// write the store (a second one goes read-only for that refresh: its
    /// files stay live and counted and are retried). `create_new` is atomic;
    /// the file holds the holder's token and its mtime is the heartbeat. A
    /// lock older than [`Index::lock_stale`] is moved away (a rename, so only
    /// one contender wins it) and taken over, so a crashed process can never
    /// wedge the index. The lock narrows the race, the content check in
    /// [`Self::commit_retired`] closes it. Returns whether this process holds
    /// the right to write.
    fn lock(&mut self) -> bool {
        if self.locked {
            return true;
        }
        let Some(dir) = self.dir.clone() else {
            self.locked = true;
            return true;
        };
        if !self.use_lock {
            self.locked = true;
            return true;
        }
        if fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let path = dir.join(RETIRED_LOCK);
        for attempt in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = file.write_all(self.lock_token.as_bytes());
                    self.lock_held = true;
                    self.locked = true;
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && self.lock_is_stale(&path) {
                        let aside = path
                            .with_file_name(format!("{RETIRED_LOCK}.stale-{}", self.lock_token));
                        if fs::rename(&path, &aside).is_ok() {
                            let _ = fs::remove_file(&aside);
                        }
                        continue;
                    }
                    return false;
                }
                Err(_) => return false,
            }
        }
        false
    }

    fn lock_is_stale(&self, path: &Path) -> bool {
        let modified = match fs::metadata(path).and_then(|m| m.modified()) {
            Ok(t) => t,
            // Gone meanwhile: nothing to wait for. Unreadable: never wedge.
            Err(_) => return true,
        };
        // A lock dated in the future (a clock that was set back) is judged by
        // its distance, not trusted forever.
        let age = match SystemTime::now().duration_since(modified) {
            Ok(age) => age,
            Err(e) => e.duration(),
        };
        age >= self.lock_stale
    }

    /// Refresh the lock's mtime while a long refresh still holds it.
    fn heartbeat(&self) {
        if self.lock_held {
            if let Some(dir) = &self.dir {
                if let Ok(file) = fs::OpenOptions::new()
                    .write(true)
                    .open(dir.join(RETIRED_LOCK))
                {
                    let _ = file.set_modified(SystemTime::now());
                }
            }
        }
    }

    /// Give the lock back, if it is still ours.
    fn unlock(&mut self) {
        self.locked = false;
        if !std::mem::take(&mut self.lock_held) {
            return;
        }
        if let Some(dir) = &self.dir {
            let path = dir.join(RETIRED_LOCK);
            if fs::read_to_string(&path).is_ok_and(|held| held == self.lock_token) {
                let _ = fs::remove_file(&path);
            }
        }
    }

    /// Another process may have written the store since this one last looked
    /// (it was not ours to write, or we were not running): take theirs.
    fn sync_retired(&mut self) {
        // Test hook: play the other process writing after this look and before
        // the commit (the window the content check in `commit_retired` covers).
        #[cfg(test)]
        if self.skip_sync {
            return;
        }
        let Some(dir) = self.dir.clone() else { return };
        let path = dir.join(RETIRED_FILE);
        // Stat first (before the read, so a change in between shows next
        // time): an unchanged length and mtime means nothing to read.
        let stamp = stamp_of(&path);
        if stamp.is_some() && stamp == self.retired_stamp {
            return;
        }
        #[cfg(test)]
        {
            self.sync_reads += 1;
        }
        if let Ok(bytes) = fs::read(&path) {
            if self.retired_seen == Some(disk::fnv1a(&bytes)) {
                self.retired_stamp = stamp;
            } else {
                self.adopt_disk_store(&dir, &bytes);
                self.retired_stamp = stamp;
            }
        }
    }

    /// `retired.json` is not the file this process loaded or wrote, so another
    /// process changed it. Writing ours over it would erase what that process
    /// folded (and may already have deleted the shards of), so ours is dropped
    /// for theirs and what was folded here but not yet written is queued to be
    /// retired again against it.
    fn adopt_disk_store(&mut self, dir: &Path, bytes: &[u8]) {
        let path = dir.join(RETIRED_FILE);
        match decode_retired(bytes, RETIRED_FORMAT, &migrate_retired) {
            Ok(retired) => {
                tracing::info!(path = %path.display(), "retired token history was changed by another process; using it");
                self.retired = retired;
                self.retired_seen = Some(disk::fnv1a(bytes));
                self.pending.append(&mut self.unsaved);
                self.retired_dirty = false;
                self.retired_urgent = false;
                self.resettle = true;
            }
            Err(StoreError::Newer(format)) => self.make_unavailable(
                &path,
                format!("written by a newer yMux (format {format})"),
                false,
            ),
            Err(StoreError::Corrupt(why)) => {
                self.make_unavailable(
                    &path,
                    format!("changed by another process and corrupt ({why})"),
                    false,
                );
            }
        }
    }

    /// Deliberately forget all retired history. NOT wired to anything: the
    /// store is the only copy of that history, so this exists for a future
    /// "reset usage history" action that asks first. The old store and its
    /// backup are moved aside as `retired.json.reset-<time>` (not deleted),
    /// so even this can be undone by hand.
    #[allow(dead_code)]
    pub fn reset_retired(&mut self) -> std::io::Result<()> {
        if let Some(dir) = &self.dir {
            for name in [RETIRED_FILE, RETIRED_BACKUP] {
                let path = dir.join(name);
                if path.exists() && set_aside(&path, "reset").is_none() {
                    return Err(std::io::Error::other("cannot move the retired store aside"));
                }
            }
        }
        self.retired = Retired::default();
        self.retired_seen = None;
        self.retired_stamp = None;
        self.unsaved.clear();
        self.retired_state = RetiredState::Ready;
        self.retired_dirty = false;
        Ok(())
    }

    /// Take a vanished file's summary out of the live set and queue it for
    /// retirement. `order` is its root's place in the spec list. A file that
    /// was never indexed is simply unknown: there is nothing to keep. Both the
    /// current summary and an older complete one that an unfinished re-parse
    /// had replaced are retired: dedup makes folding both harmless.
    fn retire(&mut self, path: &Path, order: usize) {
        self.dirty.remove(path);
        self.unreadable.remove(path);
        let mut found: Vec<FileSummary> = Vec::new();
        found.extend(self.files.remove(path));
        found.extend(self.superseded.remove(path));
        if found.is_empty() {
            found.extend(self.read_unloaded_shard(path));
        }
        let shard = self.dir.as_ref().map(|d| shard_path(d, path));
        for summary in found {
            self.pending.push(Pending {
                order,
                summary,
                shard: shard.clone(),
            });
        }
    }

    /// The shard of a vanished file that was never loaded this run (for
    /// instance one written by another version). See [`decode_shard`].
    fn read_unloaded_shard(&self, path: &Path) -> Option<FileSummary> {
        let shard = shard_path(self.dir.as_ref()?, path);
        let bytes = fs::read(&shard).ok()?;
        match self.decode_gone_shard(&shard, &bytes) {
            Gone::Summary(s) => Some(s),
            Gone::Left | Gone::Failed => None,
        }
    }

    fn decode_gone_shard(&self, shard: &Path, bytes: &[u8]) -> Gone {
        match decode_shard(bytes, self.shard_version, &self.migrator) {
            Decoded::Summary(s) => Gone::Summary(s),
            Decoded::Newer(version) => {
                tracing::warn!(shard = %shard.display(), version, "token index shard is from a newer yMux; left in place");
                Gone::Left
            }
            Decoded::Bad(why) => {
                let aside = set_aside(shard, "broken");
                tracing::warn!(shard = %shard.display(), %why, ?aside, "unusable token index shard of a deleted transcript; kept aside");
                if aside.is_some() {
                    Gone::Left
                } else {
                    Gone::Failed
                }
            }
        }
    }

    fn ensure_loaded(&mut self, path: &Path, provider: &'static str) {
        if self.files.contains_key(path) {
            return;
        }
        let Some(dir) = &self.dir else { return };
        let Ok(bytes) = fs::read(shard_path(dir, path)) else {
            return;
        };
        // The transcript exists, so a shard that cannot be used costs only a
        // re-parse: ignore it (see the module docs for the other case).
        if let Ok(summary) = serde_json::from_slice::<FileSummary>(&bytes) {
            if summary.valid_for(path, provider) {
                self.files.insert(path.to_path_buf(), summary);
                return;
            }
        }
        // An older layout that still migrates is the best complete summary of
        // the file until the re-parse finishes: keep it for a vanishing file.
        if let Decoded::Summary(old) = decode_shard(&bytes, self.shard_version, &self.migrator) {
            if old.path == path.to_string_lossy()
                && old.provider == provider
                && old.is_complete()
                && !self.superseded.contains_key(path)
            {
                self.superseded.insert(path.to_path_buf(), old);
            }
        }
    }

    fn stamp_owner(&mut self, path: &Path, owner: &str) {
        if let Some(summary) = self.files.get_mut(path) {
            if summary.owner != owner {
                owner.clone_into(&mut summary.owner);
                self.dirty.insert(path.to_path_buf());
            }
        }
        if let Some(old) = self.superseded.get_mut(path) {
            owner.clone_into(&mut old.owner);
        }
    }

    fn needs_work(&self, path: &Path, stat: Stat) -> bool {
        self.files.get(path).map_or(true, |s| {
            !(s.len == stat.len && s.mtime == stat.mtime && s.is_complete())
        })
    }

    /// Bring the index up to date with `specs` within `budget`: list or
    /// re-stat, retire what is confirmed gone, then parse what changed, newest
    /// first. `now` is the snapshot time (it decides what retired history is
    /// old enough to fold).
    pub fn refresh(&mut self, specs: &[RootSpec], budget: &mut Budget, now: u64) -> Progress {
        self.refreshes += 1;
        self.read_bytes = 0;
        let wall = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // Nothing leaves the live set unless the store that would take over
        // its history is usable.
        // And only while this process holds the lock on it: the store may have
        // been changed by another process since it was loaded.
        let ready = self.retired_ready();
        if ready && self.lock() {
            self.sync_retired();
        }
        let usable = ready && self.can_write();
        let mut all_swept = true;
        let mut todo: Vec<(Stat, PathBuf, usize)> = Vec::new();
        self.wanted = specs.iter().map(|s| (s.provider, s.dir.clone())).collect();
        // A root nobody lists any more: its files are retired one by one, each
        // only once a probe of THAT file says it is gone. A file that still
        // exists (the account was merely removed from the settings, or the
        // configuration was read wrongly) stays as it is and keeps counting.
        let stale: Vec<_> = self
            .roots
            .keys()
            .filter(|k| !self.wanted.contains(*k))
            .cloned()
            .collect();
        for key in stale {
            let Some(mut state) = self.roots.remove(&key) else {
                continue;
            };
            let due = state
                .last_sweep
                .map_or(true, |t| t.elapsed() >= self.sweep_every);
            if due && usable {
                let paths: Vec<PathBuf> = state.stats.keys().cloned().collect();
                for path in paths {
                    if (self.probe)(&path, &key.1) == Presence::Gone {
                        state.stats.remove(&path);
                        self.retire(&path, usize::MAX);
                    }
                }
                state.last_sweep = Some(Instant::now());
            }
            if !state.stats.is_empty() {
                self.roots.insert(key, state);
            }
        }
        for (si, spec) in specs.iter().enumerate() {
            let key = (spec.provider, spec.dir.clone());
            let mut state = self.roots.remove(&key).unwrap_or_default();
            let due = state
                .last_sweep
                .map_or(true, |t| t.elapsed() >= self.sweep_every);
            if due {
                let listing = (self.lister)(spec, self.file_limit);
                state.truncated = listing.truncated;
                state.incomplete = listing.incomplete;
                state.walk_skipped = listing.skipped;
                let listed: BTreeMap<PathBuf, Stat> = listing.files.into_iter().collect();
                // A file the listing does not show is gone only if a probe of
                // that file says so; the listing may be cut short.
                let missing: Vec<(PathBuf, Stat)> = state
                    .stats
                    .iter()
                    .filter(|(p, _)| !listed.contains_key(*p))
                    .map(|(p, s)| (p.clone(), *s))
                    .collect();
                let mut kept = Vec::new();
                for (path, old) in missing {
                    match (self.probe)(&path, &spec.dir) {
                        Presence::Gone if usable => self.retire(&path, si),
                        Presence::Present => {
                            let stat = fs::metadata(&path).map_or(old, |m| Stat::of(&m));
                            kept.push((path, stat));
                        }
                        _ => kept.push((path, old)),
                    }
                }
                state.stats = listed;
                state.stats.extend(kept);
                state.last_sweep = Some(Instant::now());
            } else {
                all_swept = false;
                let hot: Vec<PathBuf> = state
                    .stats
                    .iter()
                    .filter(|(path, stat)| {
                        stat.mtime_secs().saturating_add(HOT_SECS) >= wall
                            || self
                                .files
                                .get(*path)
                                .map_or(true, |s| !s.is_complete() || s.len != stat.len)
                    })
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in hot {
                    match fs::metadata(&path) {
                        Ok(meta) => {
                            state.stats.insert(path, Stat::of(&meta));
                        }
                        // NotFound alone is not proof (a missing drive says
                        // the same): probe the file against its root.
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            if usable && (self.probe)(&path, &spec.dir) == Presence::Gone {
                                state.stats.remove(&path);
                                self.retire(&path, si);
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
            for (path, stat) in &state.stats {
                self.ensure_loaded(path, spec.provider);
                self.stamp_owner(path, &spec.owner);
                if self.needs_work(path, *stat) {
                    todo.push((*stat, path.clone(), si));
                }
            }
            self.roots.insert(key, state);
        }
        todo.sort_by_key(|(stat, path, _)| (Reverse(stat.mtime), path.clone()));
        for (stat, path, si) in todo {
            if budget.spent() {
                break;
            }
            self.update(&path, specs[si].provider, stat, budget);
            self.stamp_owner(&path, &specs[si].owner);
        }
        let mut progress = Progress::default();
        for spec in specs {
            let Some(state) = self.roots.get(&(spec.provider, spec.dir.clone())) else {
                continue;
            };
            for (path, stat) in &state.stats {
                progress.total = progress.total.saturating_add(stat.len);
                if self.unreadable.contains(path) {
                    continue;
                }
                let scanned = self.files.get(path).map_or(0, |s| s.scanned.min(stat.len));
                progress.indexed = progress.indexed.saturating_add(scanned);
                progress.working |= scanned < stat.len;
            }
        }
        if all_swept && !self.gc_done && self.retire_orphans(specs, usable) && usable {
            self.gc_done = true;
        }
        self.settle(specs, now);
        self.persist(progress.working || self.was_indexing);
        self.heartbeat();
        self.persist_retired();
        if std::mem::take(&mut self.resettle) {
            // Another process's store replaced ours: redo what was folded.
            self.settle(specs, now);
            self.persist_retired();
        }
        self.unlock();
        self.was_indexing = progress.working;
        progress
    }

    /// Bring one file's summary up to `stat`, reading at most `stat.len`
    /// bytes. Returns `false` when the file could not be read (its previous
    /// summary, if it was not being extended, stays in use).
    pub fn update(
        &mut self,
        path: &Path,
        provider: &'static str,
        stat: Stat,
        budget: &mut Budget,
    ) -> bool {
        let Ok(mut file) = fs::File::open(path) else {
            self.unreadable.insert(path.to_path_buf());
            return false;
        };
        let resume = self.files.get(path).is_some_and(|s| {
            s.version == INDEX_VERSION
                && s.offset <= stat.len
                && (stat.len > s.len
                    || (stat.len == s.len && stat.mtime == s.mtime && !s.is_complete()))
                && disk::fingerprint(&mut file, s.offset).is_ok_and(|f| f == s.fingerprint)
        });
        let (mut summary, replaced) = if resume {
            (self.files.remove(path).unwrap_or_default(), None)
        } else {
            // From scratch. What it replaces stays until the new one is
            // complete (or the pass fails).
            (FileSummary::new(path, provider), self.files.remove(path))
        };
        let mut parsed = Parsed::new("unknown");
        parsed.set_context(&summary.session, &summary.project, &summary.model);
        let mut by_key: HashMap<u64, usize> = if summary.is_claude() {
            summary
                .events
                .iter()
                .enumerate()
                .map(|(at, e)| (e.k, at))
                .collect()
        } else {
            HashMap::new()
        };
        let start = summary.offset;
        let pass = disk::scan(
            &mut file,
            start,
            stat.len,
            provider,
            &mut parsed,
            budget,
            |p| {
                for event in p.events.drain(..) {
                    summary.absorb(event, &mut by_key);
                }
                summary.quotas.append(&mut p.quotas);
            },
        );
        let Ok(pass) = pass else {
            self.unreadable.insert(path.to_path_buf());
            if resume {
                // Keep what was indexed. Its bookkeeping still describes the
                // earlier pass, so the next one re-reads from `offset` and
                // absorbs the same records again, which changes nothing
                // (Claude: one entry per key; Codex: totals only count up).
                self.files.insert(path.to_path_buf(), summary);
            } else if let Some(old) = replaced {
                self.files.insert(path.to_path_buf(), old);
            }
            return false;
        };
        summary.prune_quotas();
        summary.skipped += parsed.skipped;
        let (session, project, model) = parsed.context();
        summary.session = session.to_owned();
        summary.project = project.to_owned();
        summary.model = model.to_owned();
        summary.len = stat.len;
        summary.mtime = stat.mtime;
        summary.offset = pass.offset;
        summary.scanned = pass.scanned;
        summary.fingerprint = disk::fingerprint(&mut file, pass.offset).unwrap_or(u64::MAX);
        self.read_bytes += pass.read;
        self.unreadable.remove(path);
        self.dirty.insert(path.to_path_buf());
        if summary.is_complete() {
            self.superseded.remove(path);
        } else if let Some(old) = replaced.filter(FileSummary::is_complete) {
            self.superseded.entry(path.to_path_buf()).or_insert(old);
        }
        self.files.insert(path.to_path_buf(), summary);
        true
    }

    fn persist(&mut self, force: bool) {
        #[cfg(test)]
        if self.crashed {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            self.dirty.clear();
            return;
        };
        if self.dirty.is_empty()
            || (!force
                && self
                    .last_persist
                    .is_some_and(|t| t.elapsed() < self.persist_every))
        {
            return;
        }
        self.last_persist = Some(Instant::now());
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let paths: Vec<PathBuf> = self.dirty.drain().collect();
        for path in paths {
            // A complete shard is not overwritten by an unfinished re-parse of
            // the same file: if the file vanishes before the new summary is
            // done, the old one is what a restart must find.
            if self.superseded.contains_key(&path) {
                self.dirty.insert(path);
                continue;
            }
            let Some(summary) = self.files.get(&path) else {
                continue;
            };
            let written = serde_json::to_vec(summary).ok().is_some_and(|bytes| {
                crate::durable::write_atomic(&shard_path(&dir, &path), &bytes).is_ok()
            });
            if !written {
                self.shard_write_failures += 1;
                self.dirty.insert(path);
            }
        }
    }

    #[cfg(test)]
    fn crash_at(&mut self, step: &'static str) -> bool {
        if self.crash_after == Some(step) {
            self.crashed = true;
        }
        self.crashed
    }

    /// Read the retired store from `dir` once (or again after a transient
    /// error). `true` means it is loaded and may be written.
    ///
    /// Only a plain "no such file" starts an empty store. A store that is
    /// there but unusable is never replaced, moved or deleted: it makes the
    /// retired store read-only for this process (nothing is retired, nothing
    /// is deleted, live figures are still shown, and the snapshot says so).
    /// The one exception is a store that is corrupt or missing while
    /// `retired.json.bak` is good: that is restored (the corrupt file is
    /// moved aside first), see [`Self::recover_from_backup`].
    fn retired_ready(&mut self) -> bool {
        match &self.retired_state {
            RetiredState::Ready => return true,
            RetiredState::Unavailable { retry: false, .. } => return false,
            _ => {}
        }
        let Some(dir) = self.dir.clone() else {
            self.retired_state = RetiredState::Ready;
            return true;
        };
        let path = dir.join(RETIRED_FILE);
        let stamp = stamp_of(&path);
        match fs::read(&path) {
            Ok(bytes) => match decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired) {
                Ok(retired) => {
                    self.retired = retired;
                    self.retired_seen = Some(disk::fnv1a(&bytes));
                    self.retired_stamp = stamp;
                    self.retired_state = RetiredState::Ready;
                }
                Err(StoreError::Newer(format)) => self.make_unavailable(
                    &path,
                    format!("written by a newer yMux (format {format})"),
                    false,
                ),
                Err(StoreError::Corrupt(why)) => {
                    self.recover_from_backup(&dir, Some(why));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.recover_from_backup(&dir, None);
            }
            Err(e) => self.make_unavailable(&path, format!("cannot read it: {e}"), true),
        }
        self.retired_usable()
    }

    fn make_unavailable(&mut self, path: &Path, reason: String, retry: bool) {
        if !self.warned_unavailable {
            self.warned_unavailable = true;
            tracing::warn!(path = %path.display(), %reason, "retired token history is unusable; it is left untouched and nothing will be retired or deleted until it is fixed");
        }
        self.retired_state = RetiredState::Unavailable { reason, retry };
    }

    /// `retired.json` is missing or corrupt. If `retired.json.bak` (refreshed
    /// to a verified copy before any shard was deleted) is good, continue from
    /// it: bak plus the surviving shards is complete, the shards being deleted
    /// only after a store holding them was verified and copied to bak. A
    /// corrupt main file is moved aside first (kept, never deleted); the
    /// recovered store is written back at the next commit. Otherwise a missing
    /// main with no backup is a fresh start, and anything else is unavailable.
    fn recover_from_backup(&mut self, dir: &Path, corrupt: Option<String>) {
        let main = dir.join(RETIRED_FILE);
        let backup = dir.join(RETIRED_BACKUP);
        let recovered = match fs::read(&backup) {
            Ok(bytes) => decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired).ok(),
            Err(_) => None,
        };
        match (corrupt, recovered) {
            (None, None) => {
                if backup.exists() {
                    self.make_unavailable(
                        &backup,
                        "retired.json is missing and its backup is unusable".into(),
                        false,
                    );
                } else {
                    self.retired_state = RetiredState::Ready;
                }
            }
            (Some(why), None) => {
                self.make_unavailable(&main, format!("corrupt ({why}); no usable backup"), false);
            }
            (corrupt, Some(retired)) => {
                if corrupt.is_some() && set_aside(&main, "broken").is_none() {
                    self.make_unavailable(
                        &main,
                        "corrupt, and cannot be moved aside".into(),
                        false,
                    );
                    return;
                }
                tracing::warn!(path = %main.display(), ?corrupt, "retired token history restored from retired.json.bak");
                self.retired = retired;
                self.retired_state = RetiredState::Ready;
                self.retired_dirty = true;
                self.retired_urgent = true;
            }
        }
    }

    /// Fold queued files into the retired store, raise what live files hold
    /// beyond it, and let it age.
    fn settle(&mut self, specs: &[RootSpec], now: u64) {
        if !self.can_write() {
            return;
        }
        let mut retired = std::mem::take(&mut self.retired);
        if !self.pending.is_empty() {
            // Live and pending files in the order aggregation lists them, so
            // retiring picks the same winners.
            let mut listed: Vec<(usize, &Path, &FileSummary, bool)> = Vec::new();
            for (si, spec) in specs.iter().enumerate() {
                for summary in self.root_files(spec.provider, &spec.dir) {
                    listed.push((si, Path::new(&summary.path), summary, false));
                }
            }
            for summary in self.unconfigured_files() {
                listed.push((specs.len(), Path::new(&summary.path), summary, false));
            }
            for p in &self.pending {
                listed.push((p.order, Path::new(&p.summary.path), &p.summary, true));
            }
            listed.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            let cands: Vec<Candidate> = listed
                .iter()
                .map(|&(_, _, summary, pending)| Candidate {
                    owner: &summary.owner,
                    summary,
                    pending,
                })
                .collect();
            retired.retire(&cands, now);
            drop(cands);
            drop(listed);
            for p in std::mem::take(&mut self.pending) {
                if let Some(name) = p
                    .shard
                    .as_deref()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                {
                    retired.add_pending_deletion(name.to_owned());
                }
                self.unsaved.push(p);
            }
            self.retired_dirty = true;
            self.retired_urgent = true;
        }
        let live = specs
            .iter()
            .flat_map(|spec| self.root_files(spec.provider, &spec.dir))
            .chain(self.unconfigured_files());
        if retired.upgrade_from_live(live, now) {
            self.retired_dirty = true;
        }
        if retired.compact(now) {
            self.retired_dirty = true;
        }
        self.retired = retired;
    }

    /// Persist the retired store in two phases that need no directory sync.
    ///
    /// Phase 1 writes the store (keeping the previous one as `.bak`). Phase 2,
    /// on a LATER refresh, reads it back, requires it to list and cover the
    /// retired shards, refreshes `.bak` to that verified copy, deletes the
    /// shards, and writes the store again without the list. So a shard goes
    /// only when two verified copies hold what it held.
    fn persist_retired(&mut self) {
        #[cfg(test)]
        if self.crashed {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            self.retired_dirty = false;
            self.unsaved.clear();
            return;
        };
        if !self.can_write() {
            return;
        }
        if !self.retired_dirty
            && !self.retired.pending_deletions().is_empty()
            && self.committed_at.map_or(true, |at| self.refreshes > at)
            && self
                .last_deletion_try
                .map_or(true, |t| t.elapsed() >= self.retired_persist_every)
        {
            // Reading the store back is not cheap: a shard that cannot be
            // deleted is retried at the store's own cadence, not every tick.
            self.last_deletion_try = Some(Instant::now());
            self.finish_deletions(&dir);
        }
        if !self.retired_dirty {
            return;
        }
        let due = self.retired_urgent
            || self
                .last_retired_write
                .map_or(true, |t| t.elapsed() >= self.retired_persist_every);
        if due {
            self.commit_retired(&dir);
        }
    }

    fn commit_retired(&mut self, dir: &Path) {
        if fs::create_dir_all(dir).is_err() {
            self.commit_failed();
            return;
        }
        let main = dir.join(RETIRED_FILE);
        let backup = dir.join(RETIRED_BACKUP);
        let stamp = stamp_of(&main);
        let previous = match fs::read(&main) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                self.commit_failed();
                return;
            }
        };
        // Not the file this process loaded or wrote: someone else changed it.
        // Never write over it; take it and redo what is ours against it.
        if let Some(bytes) = &previous {
            if self.retired_seen != Some(disk::fnv1a(bytes)) {
                self.adopt_disk_store(dir, bytes);
                self.retired_stamp = stamp;
                return;
            }
        }
        // The previous store, if it is good, becomes the backup.
        if let Some(previous) = previous {
            if decode_retired(&previous, RETIRED_FORMAT, &migrate_retired).is_ok()
                && crate::durable::write_atomic(&backup, &previous).is_err()
            {
                tracing::warn!(dir = %dir.display(), "cannot write the retired token history backup; will retry");
                self.commit_failed();
                return;
            }
        }
        #[cfg(test)]
        if self.crash_at("bak_written") {
            return;
        }
        let encoded = self.retired.encode();
        let written = encoded
            .as_ref()
            .is_some_and(|bytes| crate::durable::write_atomic(&main, bytes).is_ok());
        if !written {
            tracing::warn!(dir = %dir.display(), "cannot write retired token history; will retry");
            self.commit_failed();
            return;
        }
        self.retired_seen = encoded.as_deref().map(disk::fnv1a);
        self.retired_stamp = stamp_of(&main);
        self.unsaved.clear();
        self.retired_dirty = false;
        self.retired_urgent = false;
        self.last_retired_write = Some(Instant::now());
        self.committed_at = Some(self.refreshes);
        #[cfg(test)]
        self.crash_at("store_written");
    }

    /// A failed write is retried at the store's own cadence, not every tick
    /// (encoding a large store is not free).
    fn commit_failed(&mut self) {
        self.retired_urgent = false;
        self.last_retired_write = Some(Instant::now());
    }

    fn finish_deletions(&mut self, dir: &Path) {
        let main = dir.join(RETIRED_FILE);
        let Ok(bytes) = fs::read(&main) else { return };
        let Ok(disk) = decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired) else {
            return;
        };
        // Only shards that the store on disk itself lists are candidates.
        let on_disk = disk.pending_deletions();
        let names: Vec<String> = self
            .retired
            .pending_deletions()
            .into_iter()
            .filter(|n| on_disk.contains(n))
            .collect();
        if names.is_empty() {
            return;
        }
        // The backup becomes this verified copy before anything is deleted.
        let backup = dir.join(RETIRED_BACKUP);
        if fs::read(&backup).map_or(true, |b| b != bytes)
            && crate::durable::write_atomic(&backup, &bytes).is_err()
        {
            return;
        }
        #[cfg(test)]
        if self.crash_at("bak_refreshed") {
            return;
        }
        let mut done = Vec::new();
        for name in names {
            let shard = dir.join(&name);
            let bytes = match fs::read(&shard) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    done.push(name);
                    continue;
                }
                Err(_) => continue,
            };
            match decode_shard(&bytes, self.shard_version, &self.migrator) {
                // Covered by the verified store, and not a live file's cache
                // that happens to carry the same name.
                Decoded::Summary(s) if disk.covers(&s) => {
                    let live = self.files.contains_key(Path::new(&s.path))
                        || self.pending.iter().any(|p| p.summary.path == s.path);
                    if live || fs::remove_file(&shard).is_ok() {
                        done.push(name);
                    }
                }
                // Not covered (it grew after it was folded), a newer or an
                // unusable shard: not ours to delete; the orphan pass deals
                // with it. Stop listing it.
                _ => done.push(name),
            }
        }
        #[cfg(test)]
        if self.crash_at("shards_deleted") {
            return;
        }
        if self.retired.clear_pending_deletions(&done) {
            self.retired_dirty = true;
            self.retired_urgent = true;
        }
    }

    /// Once per run (retried until it succeeds): shards of files nothing lists
    /// any more are dealt with, each by what is known of ITS file:
    ///
    /// * under a configured root and confirmed gone: retired, at the place
    ///   aggregation would have listed it (so attribution does not depend on
    ///   whether the file was deleted while yMux ran or while it was closed);
    /// * under a configured root but not confirmed gone (the root is
    ///   unavailable, or the file exists and the listing missed it): held as a
    ///   live entry of that root, still counted, re-checked at every sweep;
    /// * under no configured root (the account was removed): retired, like the
    ///   stale-root branch of [`Self::refresh`].
    ///
    /// While the store is unusable nothing is retired: shards under a
    /// configured root are held (counted), the rest are left alone.
    ///
    /// Returns whether every shard could be dealt with.
    fn retire_orphans(&mut self, specs: &[RootSpec], usable: bool) -> bool {
        let Some(dir) = self.dir.clone() else {
            return true;
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            return false;
        };
        let known: HashSet<PathBuf> = self
            .roots
            .values()
            .flat_map(|s| s.stats.keys())
            .map(|p| shard_path(&dir, p))
            .collect();
        let mut shards: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_shard_name(p) && !known.contains(p))
            .collect();
        shards.sort();
        let mut complete = true;
        for shard in shards {
            let bytes = match fs::read(&shard) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let summary = match self.decode_gone_shard(&shard, &bytes) {
                Gone::Summary(s) => s,
                Gone::Left => continue,
                Gone::Failed => {
                    complete = false;
                    continue;
                }
            };
            let path = PathBuf::from(&summary.path);
            if shard_path(&dir, &path) != shard {
                let aside = set_aside(&shard, "broken");
                tracing::warn!(shard = %shard.display(), ?aside, "token index shard names another file; kept aside");
                complete &= aside.is_some();
                continue;
            }
            let owner = specs.iter().enumerate().find(|(_, spec)| {
                spec.provider == summary.provider
                    && crate::fsx::is_within(&spec.dir.to_string_lossy(), &summary.path)
            });
            match owner {
                Some((si, spec)) => match (self.probe)(&path, &spec.dir) {
                    Presence::Gone if usable => self.pending.push(Pending {
                        order: si,
                        summary,
                        shard: Some(shard),
                    }),
                    _ => self.hold(spec, path, summary),
                },
                None if usable => {
                    // The same rule as the stale-root branch of `refresh`: only
                    // a probe of THIS file that says "gone" retires it.
                    let parent = path.parent().map(Path::to_path_buf);
                    let provider = super::retired::static_provider(&summary.provider);
                    match (provider, parent) {
                        (Some(provider), Some(parent))
                            if (self.probe)(&path, &parent) != Presence::Gone =>
                        {
                            self.hold_unconfigured(provider, parent, path, summary);
                        }
                        _ => self.pending.push(Pending {
                            order: usize::MAX,
                            summary,
                            shard: Some(shard),
                        }),
                    }
                }
                None => {}
            }
        }
        complete
    }

    /// Keep a summary whose file belongs to no configured root and is not
    /// known to be gone: a live entry of a root nobody lists (counted by
    /// [`Self::unconfigured_files`]); the stale-root branch of `refresh` probes
    /// it again at the sweep cadence and retires it once confirmed gone.
    fn hold_unconfigured(
        &mut self,
        provider: &'static str,
        dir: PathBuf,
        path: PathBuf,
        summary: FileSummary,
    ) {
        let stat = Stat {
            len: summary.len,
            mtime: summary.mtime,
        };
        self.roots
            .entry((provider, dir))
            .or_default()
            .stats
            .insert(path.clone(), stat);
        self.files.entry(path).or_insert(summary);
    }

    /// Keep a summary whose file is unlisted but not known to be gone as a
    /// live entry of its root (its stat is the summary's own, so it is not
    /// re-read; the next sweep probes it again).
    fn hold(&mut self, spec: &RootSpec, path: PathBuf, summary: FileSummary) {
        let Some(state) = self.roots.get_mut(&(spec.provider, spec.dir.clone())) else {
            return;
        };
        state.stats.insert(
            path.clone(),
            Stat {
                len: summary.len,
                mtime: summary.mtime,
            },
        );
        self.files.entry(path).or_insert(summary);
    }
}
