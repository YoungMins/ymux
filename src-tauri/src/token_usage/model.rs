use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
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
}

#[derive(Debug)]
pub(super) struct Event {
    /// Index into the account list `aggregate` is given.
    pub account: usize,
    pub provider: &'static str,
    pub session: String,
    pub id: String,
    pub timestamp: u64,
    pub model: String,
    pub project: String,
    pub tokens: TokenTotals,
}
