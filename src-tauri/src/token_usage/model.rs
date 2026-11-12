use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

impl TokenTotals {
    pub fn add(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.total = self.total.saturating_add(other.total);
    }

    pub const fn difference(self, previous: Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            output: self.output.saturating_sub(previous.output),
            cache_read: self.cache_read.saturating_sub(previous.cache_read),
            cache_write: self.cache_write.saturating_sub(previous.cache_write),
            total: self.total.saturating_sub(previous.total),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quota {
    pub window_minutes: u64,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
    pub observed_at: u64,
}

#[derive(Debug, Serialize)]
pub struct ProviderUsage {
    pub id: &'static str,
    /// Owning account: `""` = default, `"none"` = home, otherwise a profile id.
    pub account: String,
    /// Other accounts that use the same directory (scanned once, here).
    pub also: Vec<String>,
    pub auth_status: &'static str,
    pub five_hour: TokenTotals,
    pub seven_day: TokenTotals,
    pub all_time: TokenTotals,
    pub quotas: Vec<Quota>,
    pub files: usize,
    pub skipped: usize,
    /// Per-card notices (`partial`, `file_limit`, `quota_unavailable`,
    /// `quota_stale`); the frontend looks up `usage.<id>_<warning>`.
    pub warnings: Vec<&'static str>,
    /// Where this card's data lives; read by the quota adapters.
    #[serde(skip)]
    pub dir: PathBuf,
    /// The literal `CLAUDE_CONFIG_DIR` of the owning account (Claude cards).
    #[serde(skip)]
    pub claude_env: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UsageRow {
    pub account: String,
    pub provider: &'static str,
    pub model: String,
    pub project: String,
    pub five_hour: TokenTotals,
    pub seven_day: TokenTotals,
    pub all_time: TokenTotals,
}

/// Tokens used by one provider during one UTC hour. The frontend groups these
/// into local calendar days, which Rust cannot do without a timezone database.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct HourBucket {
    pub hour: u64,
    pub account: String,
    pub provider: &'static str,
    pub total: u64,
}

#[derive(Debug, Serialize)]
pub struct TokenSnapshot {
    pub collected_at: u64,
    pub providers: Vec<ProviderUsage>,
    pub rows: Vec<UsageRow>,
    pub hourly: Vec<HourBucket>,
    pub warnings: Vec<String>,
    /// True while some transcript bytes are not indexed yet (first run over a
    /// large history, or a burst of new data); the figures cover what is.
    pub indexing: bool,
    /// Transcript bytes indexed so far, and the bytes in all transcripts.
    pub indexed_bytes: u64,
    pub total_bytes: u64,
    /// Transcript files that exist but could not be read (not in the figures).
    pub unreadable_files: usize,
    /// Directory entries a listing could not look at.
    pub skipped_entries: usize,
    /// Some root's listing hit the safety limit on files: the rest are not
    /// indexed (and nothing is retired on the strength of that listing).
    pub file_limit_hit: bool,
    /// Some root's listing was cut short (an unreadable folder, the limit).
    pub listing_incomplete: bool,
    /// The retired-history store could not be used: its figures are missing
    /// from this snapshot, nothing is retired or deleted, and the file is
    /// left exactly as it was.
    pub retired_unavailable: bool,
    pub retired_unavailable_reason: String,
}

#[derive(Debug)]
pub(super) struct Event {
    pub session: String,
    pub id: String,
    pub timestamp: u64,
    pub model: String,
    pub project: String,
    pub tokens: TokenTotals,
}
