//! History that outlives its transcripts.
//!
//! Claude Code deletes old transcripts (`cleanupPeriodDays`), and an account
//! directory can vanish for a while (unplugged drive, removed account). The
//! per-file index must not shrink when that happens, so the summary of a file
//! that disappears is *retired* into this store instead of being dropped.
//!
//! # What is kept, and why that is enough
//!
//! * `recent`: individual events younger than [`RECENT_SECS`] (8 days). The
//!   five-hour and seven-day windows are exact for them. Older events are
//!   folded, at most once a day, into the two aggregates below.
//! * `rollup`: all-time tokens per (account, provider, model, project) with
//!   the earliest event, which names the row's project spelling.
//! * `hours`: tokens per (UTC hour, account, provider) for the heatmap. Hours
//!   older than the heatmap window (plus a margin, see [`Retired::compact`])
//!   are pruned, so this part is bounded by the window, not by the years of
//!   history.
//! * `claude`: one record per retired Claude message id (hash): the tokens
//!   that were folded for it, the time of the event and the (account, model,
//!   project) it was folded under. A key can never be dropped: a copy can
//!   reappear in any file at any time (an unplugged account directory
//!   returns years later). The record is what makes a *later, larger* copy
//!   safe: see below. This set and `codex` are the only parts that grow with
//!   history; neither is read by the per-tick aggregation, which only looks
//!   keys up for the live files' events.
//! * `codex`: per session the largest cumulative total already counted. Codex
//!   checkpoints are cumulative, so a live checkpoint at or below it is a
//!   repeat; one above it counts only the difference, exactly as if the
//!   retired checkpoints were still in the merged stream.
//!
//! # Dedup rules
//!
//! **Claude.** A message id is counted once, with the largest value any copy
//! has ever shown (streamed updates repeat an id with growing output).
//!
//! * When a file retires, the largest value across ALL copies of each of its
//!   message ids (the retiring ones and the live ones; the first listed wins a
//!   tie, as in [`aggregate`](super::aggregate::aggregate)) is folded at once
//!   and the key is recorded. Nothing then depends on a live file surviving.
//! * A live copy of a recorded key that shows a LARGER value later
//!   ([`Retired::upgrade_from_live`]) adds the difference to the same
//!   (account, model, project) and the same event time the key was folded
//!   under, so nothing moves between accounts, rows or hours. The recorded
//!   value never goes down; an equal or smaller live copy stays suppressed.
//!
//! The invariant: for each recorded key, the tokens counted for it equal the
//! per-component sum of what was folded, and the recorded total is the
//! largest total seen.
//!
//! **Codex.** Per session, all-time equals the largest cumulative total ever
//! seen: every checkpoint is folded as the increase over the session's
//! recorded maximum, in time order. The hour of an increase is fixed when it
//! is first counted; a checkpoint that is discovered later but is at or below
//! the recorded maximum adds nothing, so it can neither double count nor
//! re-attribute. A future-dated checkpoint is counted at `now` (never
//! dropped), the same normalisation [`aggregate`](super::aggregate::aggregate)
//! applies, so figures do not jump when a file is retired.
//!
//! # Durability
//!
//! This store is the only copy of that history. It is written atomically,
//! the previous version is kept as `retired.json.bak`, and a file that cannot
//! be read is left exactly where it is (the store then turns read-only for
//! the process, see `index.rs`). [`RETIRED_FORMAT`] is independent of the live
//! shards' `INDEX_VERSION`: a bump of the latter re-parses live files but
//! never touches this.
//!
//! Retirement is two-phase and does not depend on a directory sync (there is
//! none on Windows): the shards of retired files are listed in
//! `pending_deletions` and deleted only on a LATER refresh, after the store
//! was read back from disk and found to cover them, and after `.bak` was
//! refreshed to that verified copy. So `.bak` plus the surviving shards is
//! always complete.
use super::{aggregate::HOURLY_WINDOW_SECS, index::FileSummary, model::TokenTotals};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Layout of `retired.json`. Bump it when the file's meaning changes and add
/// the upgrade to [`migrate_retired`] (and a fixture of the old layout to
/// `retired_tests`): an older store must be converted, never discarded.
pub(super) const RETIRED_FORMAT: u32 = 2;
pub(super) const RETIRED_FILE: &str = "retired.json";
/// The last store that was read back and found good.
pub(super) const RETIRED_BACKUP: &str = "retired.json.bak";
/// Events younger than this stay individual (the longest window is 7 days).
pub(super) const RECENT_SECS: u64 = 8 * 86400;
/// Events are folded in batches once they are this much past [`RECENT_SECS`],
/// so the store is rewritten for it about once a day.
const COMPACT_SLACK: u64 = 86400;
/// Hours are kept this long beyond the heatmap window.
const PRUNE_MARGIN_SECS: u64 = 30 * 86400;
/// How far past the newest known hour "now" may be trusted for pruning.
const CLOCK_SKEW_SECS: u64 = 7 * 86400;
/// Context number of a key migrated from format 1: its value and bucket are
/// unknown, so it is suppressed but never upgraded.
const FROZEN: u32 = u32::MAX;

/// One retired event, spelled out (it is young, so there are few).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct RetiredEvent {
    pub at: u64,
    pub tokens: TokenTotals,
    pub account: String,
    pub provider: String,
    pub model: String,
    pub project: String,
}

/// All-time tokens of old retired events for one row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Rollup {
    pub account: String,
    pub provider: String,
    pub model: String,
    pub project: String,
    /// `(time, total)` of the earliest folded event: the row's spelling.
    pub first_at: u64,
    pub first_total: u64,
    pub tokens: TokenTotals,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredHour {
    hour: u64,
    account: String,
    provider: String,
    total: u64,
}

/// `[key, at, ctx, input, output, cache_read, cache_write, total]`: a compact
/// array, since there is one per Claude message ever retired.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct StoredKey(u64, u64, u32, u64, u64, u64, u64, u64);

/// The on-disk spelling.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    format: u32,
    claude: Vec<StoredKey>,
    /// `(account, model, project)` per context number.
    ctxs: Vec<(String, String, String)>,
    codex: BTreeMap<String, TokenTotals>,
    recent: Vec<RetiredEvent>,
    rollup: Vec<Rollup>,
    hours: Vec<StoredHour>,
    /// Shard file names whose files were folded into this store: deleted by a
    /// later refresh once this store has been read back and found to cover
    /// them.
    pending_deletions: Vec<String>,
}

/// Format 1: bare message-id hashes.
#[derive(Default, Deserialize)]
#[serde(default)]
struct StoredV1 {
    claude_keys: Vec<u64>,
    codex: BTreeMap<String, TokenTotals>,
    recent: Vec<RetiredEvent>,
    rollup: Vec<Rollup>,
    hours: Vec<StoredHour>,
}

type RollupKey = (String, String, String, String);
type Ctx = (String, String, String);

/// What was folded for one Claude message id.
#[derive(Clone, Copy, Debug)]
struct ClaudeKey {
    tokens: TokenTotals,
    at: u64,
    ctx: u32,
}

#[derive(Default)]
pub(super) struct Retired {
    claude: HashMap<u64, ClaudeKey>,
    ctxs: Vec<Ctx>,
    ctx_at: HashMap<Ctx, u32>,
    codex: BTreeMap<String, TokenTotals>,
    recent: Vec<RetiredEvent>,
    rollup: Vec<Rollup>,
    rollup_at: HashMap<RollupKey, usize>,
    hours: BTreeMap<(u64, String, String), u64>,
    pending_deletions: BTreeSet<String>,
}

/// Why a store could not be used. Either way the file stays where it is.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StoreError {
    Corrupt(String),
    /// Written by a newer yMux than this one.
    Newer(u32),
}

/// Upgrade a store of an older [`RETIRED_FORMAT`] to the current one.
///
/// `from` is the old number, `store` the parsed JSON. When the format changes
/// again add the arm for the layout being replaced, and a fixture of it to the
/// tests (`retired_tests` fails if one is missing). A store this function
/// cannot convert makes the process read-only, so an arm is the only way not
/// to lose history.
pub(super) fn migrate_retired(from: u32, store: serde_json::Value) -> Option<Retired> {
    match from {
        1 => {
            let old: StoredV1 = serde_json::from_value(store).ok()?;
            let mut retired = Retired {
                codex: old.codex,
                recent: old.recent,
                ..Retired::default()
            };
            for key in old.claude_keys {
                retired.claude.insert(
                    key,
                    ClaudeKey {
                        tokens: TokenTotals {
                            total: u64::MAX,
                            ..TokenTotals::default()
                        },
                        at: 0,
                        ctx: FROZEN,
                    },
                );
            }
            retired.load_rollup_and_hours(old.rollup, old.hours);
            Some(retired)
        }
        _ => None,
    }
}

/// An interned name by index; a summary is checked to be consistent before it
/// is used, but data from disk must never be able to panic a release build.
fn name_at(list: &[String], at: u32) -> String {
    list.get(at as usize)
        .cloned()
        .unwrap_or_else(|| "unknown".into())
}

pub(super) fn static_provider(name: &str) -> Option<&'static str> {
    match name {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        _ => None,
    }
}

/// One file as seen by [`Retired::retire`]: listed in aggregation order.
pub(super) struct Candidate<'a> {
    pub owner: &'a str,
    pub summary: &'a FileSummary,
    /// True for a file that is being retired, false for one still live.
    pub pending: bool,
}

impl Retired {
    pub fn has_claude(&self, key: u64) -> bool {
        self.claude.contains_key(&key)
    }

    /// Largest cumulative total of a Codex session already counted.
    pub fn previous(&self, session: &str) -> TokenTotals {
        self.codex.get(session).copied().unwrap_or_default()
    }

    pub fn recent(&self) -> &[RetiredEvent] {
        &self.recent
    }

    pub fn rollup(&self) -> &[Rollup] {
        &self.rollup
    }

    /// Heatmap hours from `from_hour` on.
    pub fn hours_from(&self, from_hour: u64) -> impl Iterator<Item = (u64, &str, &str, u64)> {
        self.hours
            .range((from_hour, String::new(), String::new())..)
            .map(|((hour, account, provider), total)| {
                (*hour, account.as_str(), provider.as_str(), *total)
            })
    }

    pub fn pending_deletions(&self) -> Vec<String> {
        self.pending_deletions.iter().cloned().collect()
    }

    pub fn add_pending_deletion(&mut self, shard_name: String) {
        self.pending_deletions.insert(shard_name);
    }

    /// Forget `names`; returns whether anything was listed.
    pub fn clear_pending_deletions(&mut self, names: &[String]) -> bool {
        let before = self.pending_deletions.len();
        for name in names {
            self.pending_deletions.remove(name);
        }
        self.pending_deletions.len() != before
    }

    /// What one aggregation reads of the retired history besides key lookups:
    /// bounded by the windows and by the distinct rows, never by the number of
    /// events that were retired.
    #[cfg(test)]
    pub fn work_units(&self) -> usize {
        self.recent.len() + self.rollup.len() + self.hours.len()
    }

    #[cfg(test)]
    pub fn key_count(&self) -> usize {
        self.claude.len()
    }

    /// Every checkpoint and message of `summary` is already counted here at
    /// least as large as it is: deleting the summary's shard loses nothing.
    pub fn covers(&self, summary: &FileSummary) -> bool {
        if summary.is_claude() {
            summary
                .events
                .iter()
                .all(|e| self.claude.get(&e.k).is_some_and(|k| k.tokens.total >= e.t))
        } else {
            summary.events.iter().all(|e| {
                summary
                    .sessions
                    .get(e.n as usize)
                    .is_some_and(|s| self.previous(s).total >= e.t)
            })
        }
    }

    fn ctx_id(&mut self, account: &str, model: &str, project: &str) -> u32 {
        let ctx = (account.to_owned(), model.to_owned(), project.to_owned());
        if let Some(&id) = self.ctx_at.get(&ctx) {
            return id;
        }
        let id = u32::try_from(self.ctxs.len()).unwrap_or(FROZEN - 1);
        self.ctxs.push(ctx.clone());
        self.ctx_at.insert(ctx, id);
        id
    }

    fn add_event(&mut self, event: RetiredEvent, now: u64) {
        if event.at.saturating_add(RECENT_SECS) < now {
            self.fold(&event);
        } else {
            self.recent.push(event);
        }
    }

    fn fold(&mut self, event: &RetiredEvent) {
        let key = (
            event.account.clone(),
            event.provider.clone(),
            event.model.clone(),
            event.project.clone(),
        );
        let at = match self.rollup_at.get(&key) {
            Some(&at) => at,
            None => {
                self.rollup.push(Rollup {
                    account: event.account.clone(),
                    provider: event.provider.clone(),
                    model: event.model.clone(),
                    project: event.project.clone(),
                    first_at: event.at,
                    first_total: event.tokens.total,
                    tokens: TokenTotals::default(),
                });
                self.rollup_at.insert(key, self.rollup.len() - 1);
                self.rollup.len() - 1
            }
        };
        let row = &mut self.rollup[at];
        if (event.at, event.tokens.total) < (row.first_at, row.first_total) {
            row.first_at = event.at;
            row.first_total = event.tokens.total;
        }
        row.tokens.add(event.tokens);
        if event.tokens.total > 0 {
            let bucket = self
                .hours
                .entry((
                    event.at / 3600,
                    event.account.clone(),
                    event.provider.clone(),
                ))
                .or_default();
            *bucket = bucket.saturating_add(event.tokens.total);
        }
    }

    /// Newest instant the store has data for, trusted only up to `now`.
    fn newest_known(&self) -> u64 {
        let hours = self
            .hours
            .keys()
            .next_back()
            .map_or(0, |(h, ..)| h.saturating_mul(3600).saturating_add(3599));
        let recent = self.recent.iter().map(|e| e.at).max().unwrap_or(0);
        hours.max(recent)
    }

    /// Fold events that left the exact window and drop heatmap hours that
    /// left the heatmap. Returns whether anything changed.
    ///
    /// Folding is lossless for all-time and the heatmap, so it follows `now`.
    /// Pruning is not, so it follows `min(now, newest known data + a week)`
    /// and keeps a month beyond the window: a clock that jumped forward
    /// cannot prune history that is really recent.
    pub fn compact(&mut self, now: u64) -> bool {
        let mut changed = false;
        let due = now.saturating_sub(RECENT_SECS + COMPACT_SLACK);
        if self.recent.iter().any(|e| e.at < due) {
            let cutoff = now.saturating_sub(RECENT_SECS);
            for event in std::mem::take(&mut self.recent) {
                if event.at < cutoff {
                    self.fold(&event);
                } else {
                    self.recent.push(event);
                }
            }
            changed = true;
        }
        let reference = now.min(self.newest_known().saturating_add(CLOCK_SKEW_SECS));
        let oldest = (reference.saturating_sub(HOURLY_WINDOW_SECS + PRUNE_MARGIN_SECS) / 3600)
            .saturating_sub(1);
        if self.hours.keys().next().is_some_and(|(h, ..)| *h < oldest) {
            self.hours = self
                .hours
                .split_off(&(oldest, String::new(), String::new()));
            changed = true;
        }
        changed
    }

    /// Retire every `pending` candidate. `cands` lists live and pending files
    /// together in the order [`aggregate`](super::aggregate::aggregate) sees
    /// them, so each message is folded with the copy aggregation was counting.
    pub fn retire(&mut self, cands: &[Candidate], now: u64) {
        self.retire_claude(cands, now);
        self.retire_codex(cands, now);
    }

    fn event_of(cand: &Candidate, at: u64, tokens: TokenTotals, m: u32, p: u32) -> RetiredEvent {
        let s = cand.summary;
        RetiredEvent {
            at,
            tokens,
            account: cand.owner.to_owned(),
            provider: s.provider.clone(),
            model: name_at(&s.models, m),
            project: name_at(&s.projects, p),
        }
    }

    /// Add `tokens` (the increase over the recorded value) to key `k`.
    fn raise_claude(&mut self, k: u64, tokens: TokenTotals, now: u64) -> bool {
        let Some(rec) = self.claude.get(&k).copied() else {
            return false;
        };
        if rec.ctx == FROZEN || tokens.total <= rec.tokens.total {
            return false;
        }
        let Some((account, model, project)) = self.ctxs.get(rec.ctx as usize).cloned() else {
            return false;
        };
        let diff = tokens.difference(rec.tokens);
        if let Some(rec) = self.claude.get_mut(&k) {
            rec.tokens.add(diff);
        }
        self.add_event(
            RetiredEvent {
                at: rec.at,
                tokens: diff,
                account,
                provider: "claude".into(),
                model,
                project,
            },
            now,
        );
        true
    }

    fn retire_claude(&mut self, cands: &[Candidate], now: u64) {
        let wanted: HashSet<u64> = cands
            .iter()
            .filter(|c| c.pending && c.summary.is_claude())
            .flat_map(|c| c.summary.events.iter().map(|e| e.k))
            .collect();
        if wanted.is_empty() {
            return;
        }
        // The copy aggregation would count for each key: largest record, the
        // first listed wins a tie. Live and retiring copies compete alike.
        let mut best: HashMap<u64, (u64, usize, usize)> = HashMap::new();
        for (ci, cand) in cands.iter().enumerate() {
            if !cand.summary.is_claude() {
                continue;
            }
            for (ei, e) in cand.summary.events.iter().enumerate() {
                if !wanted.contains(&e.k) {
                    continue;
                }
                if best.get(&e.k).map_or(true, |&(t, ..)| e.t > t) {
                    best.insert(e.k, (e.t, ci, ei));
                }
            }
        }
        let mut order: Vec<(u64, usize, usize)> = best
            .into_iter()
            .map(|(k, (_, ci, ei))| (k, ci, ei))
            .collect();
        order.sort_unstable_by_key(|&(k, ..)| k);
        for (k, ci, ei) in order {
            let cand = &cands[ci];
            let e = &cand.summary.events[ei];
            if self.claude.contains_key(&k) {
                self.raise_claude(k, e.tokens(), now);
            } else {
                let s = cand.summary;
                let ctx = self.ctx_id(
                    cand.owner,
                    &name_at(&s.models, e.m),
                    &name_at(&s.projects, e.p),
                );
                self.claude.insert(
                    k,
                    ClaudeKey {
                        tokens: e.tokens(),
                        at: e.at,
                        ctx,
                    },
                );
                let event = Self::event_of(cand, e.at, e.tokens(), e.m, e.p);
                self.add_event(event, now);
            }
        }
    }

    /// Fold what live files hold beyond what was recorded for their keys (a
    /// larger copy that was indexed after the smaller one retired). Returns
    /// whether anything changed.
    pub fn upgrade_from_live<'a>(
        &mut self,
        files: impl Iterator<Item = &'a FileSummary>,
        now: u64,
    ) -> bool {
        let mut changed = false;
        for file in files {
            if !file.is_claude() {
                continue;
            }
            for e in &file.events {
                if self
                    .claude
                    .get(&e.k)
                    .is_some_and(|rec| e.t > rec.tokens.total)
                {
                    changed |= self.raise_claude(e.k, e.tokens(), now);
                }
            }
        }
        changed
    }

    fn retire_codex(&mut self, cands: &[Candidate], now: u64) {
        let touched: HashSet<&str> = cands
            .iter()
            .filter(|c| c.pending && !c.summary.is_claude())
            .flat_map(|c| {
                c.summary
                    .events
                    .iter()
                    .filter_map(|e| c.summary.sessions.get(e.n as usize).map(String::as_str))
            })
            .collect();
        if touched.is_empty() {
            return;
        }
        // The whole merged stream of each touched session, live checkpoints
        // included: they are folded too, so the retired total never runs
        // behind a checkpoint that is still in a live file.
        let mut streams: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
        for (ci, cand) in cands.iter().enumerate() {
            if cand.summary.is_claude() {
                continue;
            }
            for (ei, e) in cand.summary.events.iter().enumerate() {
                let Some(session) = cand.summary.sessions.get(e.n as usize) else {
                    continue;
                };
                if touched.contains(session.as_str()) {
                    streams.entry(session).or_default().push((ci, ei));
                }
            }
        }
        for (session, mut stream) in streams {
            stream.sort_by_key(|&(ci, ei)| {
                let e = &cands[ci].summary.events[ei];
                (e.at, e.t)
            });
            let mut previous = self.previous(session);
            for (ci, ei) in stream {
                let cand = &cands[ci];
                let e = &cand.summary.events[ei];
                let totals = e.tokens();
                if totals.total <= previous.total {
                    continue;
                }
                let delta = totals.difference(previous);
                previous = totals;
                // A future-dated checkpoint is counted now, never dropped
                // (aggregation clamps the same way).
                let event = Self::event_of(cand, e.at.min(now), delta, e.m, e.p);
                self.add_event(event, now);
            }
            self.codex.insert(session.to_owned(), previous);
        }
    }

    pub fn encode(&self) -> Option<Vec<u8>> {
        let mut claude: Vec<StoredKey> = self
            .claude
            .iter()
            .map(|(k, rec)| {
                StoredKey(
                    *k,
                    rec.at,
                    rec.ctx,
                    rec.tokens.input,
                    rec.tokens.output,
                    rec.tokens.cache_read,
                    rec.tokens.cache_write,
                    rec.tokens.total,
                )
            })
            .collect();
        claude.sort_unstable_by_key(|k| k.0);
        let stored = Stored {
            format: RETIRED_FORMAT,
            claude,
            ctxs: self.ctxs.clone(),
            codex: self.codex.clone(),
            recent: self.recent.clone(),
            rollup: self.rollup.clone(),
            hours: self
                .hours
                .iter()
                .map(|((hour, account, provider), total)| StoredHour {
                    hour: *hour,
                    account: account.clone(),
                    provider: provider.clone(),
                    total: *total,
                })
                .collect(),
            pending_deletions: self.pending_deletions.iter().cloned().collect(),
        };
        serde_json::to_vec(&stored).ok()
    }

    fn load_rollup_and_hours(&mut self, rollup: Vec<Rollup>, hours: Vec<StoredHour>) {
        for row in rollup {
            let key = (
                row.account.clone(),
                row.provider.clone(),
                row.model.clone(),
                row.project.clone(),
            );
            self.rollup_at.insert(key, self.rollup.len());
            self.rollup.push(row);
        }
        for h in hours {
            let bucket = self
                .hours
                .entry((h.hour, h.account, h.provider))
                .or_default();
            *bucket = bucket.saturating_add(h.total);
        }
    }

    fn from_stored(stored: Stored) -> Result<Self, StoreError> {
        let mut retired = Self {
            codex: stored.codex,
            recent: stored.recent,
            pending_deletions: stored.pending_deletions.into_iter().collect(),
            ..Self::default()
        };
        for (id, ctx) in stored.ctxs.into_iter().enumerate() {
            retired.ctx_at.insert(ctx.clone(), id as u32);
            retired.ctxs.push(ctx);
        }
        for StoredKey(k, at, ctx, input, output, cache_read, cache_write, total) in stored.claude {
            if ctx != FROZEN && ctx as usize >= retired.ctxs.len() {
                return Err(StoreError::Corrupt(format!("key {k} names context {ctx}")));
            }
            retired.claude.insert(
                k,
                ClaudeKey {
                    tokens: TokenTotals {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        total,
                    },
                    at,
                    ctx,
                },
            );
        }
        retired.load_rollup_and_hours(stored.rollup, stored.hours);
        Ok(retired)
    }
}

/// Parse a store written by [`Retired::encode`], converting an older format
/// with `migrate`. `current` is a parameter so tests can play a future
/// version; the app passes [`RETIRED_FORMAT`] and [`migrate_retired`].
pub(super) fn decode_retired(
    bytes: &[u8],
    current: u32,
    migrate: &dyn Fn(u32, serde_json::Value) -> Option<Retired>,
) -> Result<Retired, StoreError> {
    #[derive(Deserialize)]
    struct Probe {
        format: u32,
    }
    let probe: Probe =
        serde_json::from_slice(bytes).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    match probe.format {
        f if f == current => serde_json::from_slice::<Stored>(bytes)
            .map_err(|e| StoreError::Corrupt(e.to_string()))
            .and_then(Retired::from_stored),
        f if f > current => Err(StoreError::Newer(f)),
        0 => Err(StoreError::Corrupt("no format number".into())),
        f => {
            let value =
                serde_json::from_slice(bytes).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            migrate(f, value)
                .ok_or_else(|| StoreError::Corrupt(format!("no migration from format {f}")))
        }
    }
}
