//! Daily webhook digest, pure part: which local day is due, the day's
//! numbers from the activity log, and the `digest` delivery event.
//!
//! No prompt or assistant text is ever read here: a digest is built from
//! counts, names the user chose (workspaces, accounts, agents) and the
//! per-turn diff stats the activity log already holds. Every string still goes
//! through [`crate::activity::redact::redact`] and is clipped, and the payload
//! builders clean and clip again.
//!
//! Local time is a fixed UTC offset in minutes passed in by the caller
//! ("DST-agnostic": the offset in force *now* is applied to the whole day, so
//! the day's edges can be an hour off in the one night a year the offset
//! changes). The day files of the activity store are UTC days, so a local day
//! is cut out of them by timestamp, not by file.
//!
//! The scheduling decision is [`next_due`]; the desktop glue
//! (`webhooks::runtime`) calls it from the activity worker's minute tick,
//! builds on a short-lived thread, and persists [`State`] with
//! `crate::durable`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::DeliveryEvent;
use crate::activity::redact::{clip, redact};
use crate::activity::store::{day_number, day_string, kind, Record};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// Most entries in each of the lists a digest carries.
const TOP: usize = 5;
/// Longest name kept.
const NAME_MAX: usize = 60;

// ------------------------------------------------------------------ calendar

/// Days since the Unix epoch of the local calendar day containing `now_ms`.
pub fn local_day(now_ms: i64, offset_min: i32) -> i64 {
    (now_ms + i64::from(offset_min) * 60_000).div_euclid(DAY_MS)
}

/// Local hour of day (0-23) of `now_ms`.
pub fn local_hour(now_ms: i64, offset_min: i32) -> u8 {
    let ms = (now_ms + i64::from(offset_min) * 60_000).rem_euclid(DAY_MS);
    u8::try_from(ms / HOUR_MS).unwrap_or(0)
}

/// `YYYY-MM-DD` of a day number (days since the epoch).
pub fn day_name(day: i64) -> String {
    day_string(u64::try_from(day.saturating_mul(DAY_MS)).unwrap_or(0))
}

/// The UTC millisecond range `[start, end)` of local day `day`.
pub fn day_range(day: i64, offset_min: i32) -> (i64, i64) {
    let start = day * DAY_MS - i64::from(offset_min) * 60_000;
    (start, start + DAY_MS)
}

/// The current offset of the machine's local time zone from UTC, in minutes
/// (east is positive). `0` if the OS cannot say.
#[cfg(unix)]
pub fn local_offset_minutes(now_ms: i64) -> i32 {
    let t: libc::time_t = (now_ms / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the zeroed `tm` we own and reads
    // the `time_t` we pass by reference.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        tm
    };
    // `tm_gmtoff` is `c_long`: i64 on LP64 (the conversion is then an identity
    // clippy flags) but i32 on ILP32, where it is needed.
    #[allow(clippy::useless_conversion)]
    let secs = i64::from(tm.tm_gmtoff);
    i32::try_from(secs / 60).unwrap_or(0)
}

#[cfg(windows)]
pub fn local_offset_minutes(_now_ms: i64) -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    // SAFETY: `tz` is a valid out-parameter.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    // TIME_ZONE_ID_STANDARD = 1, TIME_ZONE_ID_DAYLIGHT = 2, failure = u32::MAX.
    let extra = match id {
        1 => tz.StandardBias,
        2 => tz.DaylightBias,
        u32::MAX => return 0,
        _ => 0,
    };
    // `Bias` is minutes *west* of UTC.
    -(tz.Bias + extra)
}

// ------------------------------------------------------------------ schedule

/// Longest stretch a scheduled digest reaches back: a laptop closed for a
/// week sends the last two days, not a backfill.
pub const MAX_LOOKBACK_MS: i64 = 2 * DAY_MS;

/// Persisted next to the webhook queues (never in `config.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// UTC ms at which the newest scheduled digest's period ended. `0` =
    /// never (or only a legacy `last_sent` was recorded).
    pub period_end_ms: i64,
    /// Legacy shape: local date (`YYYY-MM-DD`) of the newest handled day.
    /// Read for migration, never written.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_sent: String,
}

impl State {
    /// Where the previous scheduled digest ended, if one ever ran. A legacy
    /// date counts as "ended at that date's digest hour".
    pub fn period_end(&self, offset_min: i32, digest_hour: u8) -> Option<i64> {
        if self.period_end_ms > 0 {
            return Some(self.period_end_ms);
        }
        let day = day_number(&self.last_sent)?;
        Some(day_range(day, offset_min).0 + i64::from(digest_hour.min(23)) * HOUR_MS)
    }
}

/// A span of time a digest covers, UTC ms, `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// How long a scheduled check that found its period empty is not repeated.
pub const EMPTY_BACKOFF_MS: i64 = 30 * 60_000;

/// True while a scheduled build that came back empty at `empty_at_ms` (`0` =
/// none) should not be repeated: under [`EMPTY_BACKOFF_MS`] ago and still the
/// same local day.
pub fn in_empty_backoff(now_ms: i64, offset_min: i32, empty_at_ms: i64) -> bool {
    empty_at_ms > 0
        && now_ms >= empty_at_ms
        && now_ms - empty_at_ms < EMPTY_BACKOFF_MS
        && local_day(now_ms, offset_min) == local_day(empty_at_ms, offset_min)
}

/// The period a scheduled digest should cover right now, if any.
///
/// At most one per local day, and only once the local hour has reached
/// `digest_hour`. It runs from the end of the previous scheduled digest (or,
/// for the very first one, local midnight today) up to `now_ms`, never more
/// than [`MAX_LOOKBACK_MS`] back, so nothing between two digests is skipped.
/// The caller records `end_ms` in [`State`] only after it sent; an empty
/// period advances nothing, so a later check the same day may still send.
pub fn next_due(
    now_ms: i64,
    offset_min: i32,
    digest_hour: u8,
    prev_end: Option<i64>,
) -> Option<Period> {
    let today = local_day(now_ms, offset_min);
    if local_hour(now_ms, offset_min) < digest_hour.min(23) {
        return None;
    }
    let start = match prev_end {
        Some(end) => {
            if local_day(end, offset_min) >= today {
                return None;
            }
            end
        }
        None => day_range(today, offset_min).0,
    };
    let start = start.max(now_ms - MAX_LOOKBACK_MS);
    (start < now_ms).then_some(Period {
        start_ms: start,
        end_ms: now_ms,
    })
}

/// `YYYY-MM-DD HH:MM` of `ms` on the local clock.
pub fn local_stamp(ms: i64, offset_min: i32) -> String {
    let minute = (ms + i64::from(offset_min) * 60_000).rem_euclid(DAY_MS) / 60_000;
    format!(
        "{} {:02}:{:02}",
        day_name(local_day(ms, offset_min)),
        minute / 60,
        minute % 60
    )
}

// ------------------------------------------------------------------- content

/// One token line: a day's total for an account and provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenLine {
    pub account: String,
    pub provider: String,
    pub total: u64,
}

/// A name and a count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counted {
    pub name: String,
    pub count: u32,
}

/// What a digest says. All numbers; names are user-chosen labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Digest {
    /// Local date the digest was built on (the end of its period).
    pub date: String,
    /// Local start and end (`YYYY-MM-DD HH:MM`) of a scheduled digest's
    /// period; empty for a "today so far" test send.
    pub from: String,
    pub to: String,
    /// The day is still running (a "so far" digest).
    pub partial: bool,
    pub turns: u32,
    pub failed_turns: u32,
    pub sessions: u32,
    pub agents: Vec<Counted>,
    pub projects: Vec<Counted>,
    pub tool_calls: u32,
    pub tool_failures: u32,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub tokens: Vec<TokenLine>,
    pub tokens_total: u64,
    /// The token index was still building, so no token figures are given.
    pub tokens_unavailable: bool,
}

impl Digest {
    /// A day without a single turn or tool call is not worth a message.
    pub fn is_empty(&self) -> bool {
        self.turns == 0 && self.tool_calls == 0 && self.tool_failures == 0
    }
}

/// Tokens one provider used in one UTC hour (mirrors
/// `token_usage::HourBucket`, so this module stays free of that one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourTokens {
    /// Hours since the epoch (UTC).
    pub hour: u64,
    pub account: String,
    pub provider: String,
    pub total: u64,
}

/// Display names, from backend state.
pub struct Names<'a> {
    pub workspace: &'a dyn Fn(u32) -> String,
    pub account: &'a dyn Fn(&str) -> String,
}

fn name(s: &str) -> String {
    clip(&redact(s.trim()), NAME_MAX)
}

fn top(counts: BTreeMap<String, u32>) -> Vec<Counted> {
    let mut v: Vec<Counted> = counts
        .into_iter()
        .map(|(name, count)| Counted { name, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    v.truncate(TOP);
    v
}

fn u32_of(v: Option<&Value>) -> u32 {
    v.and_then(Value::as_u64)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// Build the digest of the records whose timestamp is in `[start_ms, end_ms)`
/// and the token hours that start in it. Reads counts and short labels only:
/// `Record::data` fields other than the numbers below are never touched.
pub fn build(
    date: &str,
    partial: bool,
    (start_ms, end_ms): (i64, i64),
    records: &[Record],
    hours: &[HourTokens],
    names: &Names<'_>,
) -> Digest {
    let mut d = Digest {
        date: date.to_string(),
        partial,
        ..Digest::default()
    };
    let mut sessions: BTreeSet<&str> = BTreeSet::new();
    let mut agents: BTreeMap<String, u32> = BTreeMap::new();
    let mut projects: BTreeMap<String, u32> = BTreeMap::new();
    for r in records {
        let ts = i64::try_from(r.ts).unwrap_or(i64::MAX);
        if ts < start_ms || ts >= end_ms {
            continue;
        }
        match r.kind.as_str() {
            kind::TURN_END => {
                d.turns = d.turns.saturating_add(1);
                let key = if r.session.is_empty() {
                    r.pane.as_str()
                } else {
                    r.session.as_str()
                };
                sessions.insert(key);
                if !r.agent.is_empty() {
                    *agents.entry(name(&r.agent)).or_default() += 1;
                }
                let ws = (names.workspace)(r.ws);
                if !ws.trim().is_empty() {
                    *projects.entry(name(&ws)).or_default() += 1;
                }
                if r.data.pointer("/outcome/status").and_then(Value::as_str) == Some("failed") {
                    d.failed_turns = d.failed_turns.saturating_add(1);
                }
                d.tool_calls = d
                    .tool_calls
                    .saturating_add(u32_of(r.data.get("tool_count")));
                if let Some(s) = r.data.get("stats") {
                    d.files_changed = d.files_changed.saturating_add(u32_of(s.get("files")));
                    d.insertions = d.insertions.saturating_add(u32_of(s.get("insertions")));
                    d.deletions = d.deletions.saturating_add(u32_of(s.get("deletions")));
                }
            }
            kind::TOOL => {
                let failed = r
                    .data
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_some_and(|c| c != 0);
                if failed {
                    d.tool_failures = d.tool_failures.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    d.sessions = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
    d.agents = top(agents);
    d.projects = top(projects);
    let mut tokens: BTreeMap<(String, String), u64> = BTreeMap::new();
    for h in hours {
        let at = i64::try_from(h.hour)
            .unwrap_or(i64::MAX)
            .saturating_mul(HOUR_MS);
        if at < start_ms || at >= end_ms || h.total == 0 {
            continue;
        }
        let acc = (names.account)(&h.account);
        let entry = tokens.entry((name(&acc), name(&h.provider))).or_default();
        *entry = entry.saturating_add(h.total);
    }
    d.tokens_total = tokens.values().fold(0u64, |a, t| a.saturating_add(*t));
    let mut lines: Vec<TokenLine> = tokens
        .into_iter()
        .map(|((account, provider), total)| TokenLine {
            account,
            provider,
            total,
        })
        .collect();
    lines.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.account.cmp(&b.account))
    });
    lines.truncate(TOP);
    d.tokens = lines;
    d
}

fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn list(items: &[Counted]) -> String {
    items
        .iter()
        .map(|c| format!("{} ({})", c.name, c.count))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The delivery event for `digest`. `test` marks a manual test send.
pub fn to_event(digest: &Digest, now_ms: i64, test: bool) -> DeliveryEvent {
    let d = digest;
    let mut bullets: Vec<String> = Vec::new();
    let failed = if d.failed_turns > 0 {
        format!(", {} failed", d.failed_turns)
    } else {
        String::new()
    };
    bullets.push(format!(
        "{}{failed} in {}",
        plural(d.turns, "turn", "turns"),
        plural(d.sessions, "session", "sessions")
    ));
    if !d.agents.is_empty() {
        bullets.push(format!("Agents: {}", list(&d.agents)));
    }
    if d.tool_calls > 0 || d.tool_failures > 0 {
        bullets.push(format!(
            "Tools: {} calls, {} failed",
            d.tool_calls, d.tool_failures
        ));
    }
    if d.files_changed > 0 || d.insertions > 0 || d.deletions > 0 {
        bullets.push(format!(
            "Changes: {} (+{}/-{})",
            plural(d.files_changed, "file", "files"),
            d.insertions,
            d.deletions
        ));
    }
    if d.tokens_unavailable {
        bullets.push("Tokens: totals unavailable (indexing history)".to_string());
    } else if d.tokens_total > 0 {
        let lines = d
            .tokens
            .iter()
            .map(|t| format!("{}/{} {}", t.account, t.provider, compact(t.total)))
            .collect::<Vec<_>>()
            .join(", ");
        bullets.push(format!("Tokens: {} ({lines})", compact(d.tokens_total)));
    }
    if !d.projects.is_empty() {
        bullets.push(format!("Top workspaces: {}", list(&d.projects)));
    }
    let mut headline = if !d.from.is_empty() && !d.to.is_empty() {
        format!("{} to {}", d.from, d.to)
    } else {
        d.date.clone()
    };
    if d.partial {
        headline.push_str(" (so far)");
    }
    if test {
        headline = format!("[test send] {headline}");
    }
    DeliveryEvent {
        kind: "digest".to_string(),
        ts: now_ms,
        agent: "ymux".to_string(),
        headline,
        bullets,
        digest: Some(d.clone()),
        ..DeliveryEvent::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 2026-11-12 00:00:00 UTC.
    const NOV12: i64 = 1_794_441_600_000;

    fn at(day_offset_h: i64) -> i64 {
        NOV12 + day_offset_h * HOUR_MS
    }

    #[test]
    fn local_calendar_follows_the_offset() {
        // 23:30 UTC on the 12th is already the 13th at +09:00, still the
        // 12th at -05:00.
        let t = at(23) + 30 * 60_000;
        assert_eq!(day_name(local_day(t, 0)), "2026-11-12");
        assert_eq!(day_name(local_day(t, 540)), "2026-11-13");
        assert_eq!(local_hour(t, 540), 8);
        assert_eq!(day_name(local_day(t, -300)), "2026-11-12");
        assert_eq!(local_hour(t, -300), 18);
        // Before the epoch's first day still divides correctly.
        assert_eq!(local_day(-1, 0), -1);
        assert_eq!(local_hour(-1, 0), 23);
    }

    #[test]
    fn day_range_is_the_local_day_in_utc() {
        let (s, e) = day_range(local_day(at(10), 540), 540);
        assert_eq!(e - s, DAY_MS);
        // +09:00: local midnight of the 12th is 15:00 UTC on the 11th.
        assert_eq!(s, NOV12 - 9 * HOUR_MS);
    }

    #[test]
    fn empty_backoff_holds_for_30_minutes_within_the_same_day() {
        let t = 10 * DAY_MS + 9 * HOUR_MS;
        assert!(!in_empty_backoff(t, 0, 0), "no prior empty run");
        assert!(in_empty_backoff(t + 60_000, 0, t));
        assert!(in_empty_backoff(t + EMPTY_BACKOFF_MS - 1, 0, t));
        assert!(!in_empty_backoff(t + EMPTY_BACKOFF_MS, 0, t));
        assert!(!in_empty_backoff(t - 1, 0, t), "clock went backwards");
        // A new local day resets it even inside the 30 minutes.
        let late = 11 * DAY_MS - 60_000;
        assert!(!in_empty_backoff(late + 120_000, 0, late));
    }

    #[test]
    fn nothing_is_due_before_the_digest_hour() {
        assert_eq!(next_due(at(17), 0, 18, Some(at(-6))), None);
        assert_eq!(next_due(at(17), 0, 18, None), None);
        let p = next_due(at(18), 0, 18, Some(at(-6))).unwrap();
        assert_eq!((p.start_ms, p.end_ms), (at(-6), at(18)));
    }

    #[test]
    fn a_period_runs_from_the_previous_end_so_nothing_is_skipped() {
        let prev = at(-6);
        let p = next_due(at(19), 0, 18, Some(prev)).unwrap();
        assert_eq!(p.start_ms, prev);
        assert_eq!(p.end_ms, at(19));
        // Chained: the next period starts exactly where this one ended.
        let q = next_due(at(24 + 19), 0, 18, Some(p.end_ms)).unwrap();
        assert_eq!(q.start_ms, p.end_ms);
    }

    #[test]
    fn at_most_one_scheduled_send_per_local_day() {
        assert_eq!(next_due(at(20), 0, 18, Some(at(18))), None);
        assert_eq!(next_due(at(23), 0, 18, Some(at(19))), None);
        // A period end from the future (clock moved back) sends nothing.
        assert_eq!(next_due(at(20), 0, 18, Some(at(100))), None);
    }

    #[test]
    fn the_lookback_is_capped_at_48_hours() {
        let p = next_due(at(19), 0, 18, Some(at(-24 * 10))).unwrap();
        assert_eq!(p.start_ms, at(19) - MAX_LOOKBACK_MS);
    }

    #[test]
    fn the_first_digest_starts_at_local_midnight() {
        let p = next_due(at(19), 0, 18, None).unwrap();
        assert_eq!(p.start_ms, at(0));
        // +09:00: local midnight of the 12th is 15:00 UTC on the 11th.
        let p = next_due(at(10), 540, 18, None).unwrap();
        assert_eq!(p.start_ms, NOV12 - 9 * HOUR_MS);
    }

    #[test]
    fn the_hour_is_local_and_clamped() {
        // 10:00 UTC is 19:00 at +09:00: due; and handled once ended today.
        assert!(next_due(at(10), 540, 18, Some(at(-20))).is_some());
        assert_eq!(next_due(at(10), 540, 18, Some(at(9))), None);
        // Hour 99 behaves as 23.
        assert_eq!(next_due(at(22), 0, 99, Some(at(-6))), None);
        assert!(next_due(at(23), 0, 99, Some(at(-6))).is_some());
    }

    #[test]
    fn a_legacy_date_state_means_ended_at_that_days_digest_hour() {
        let s: State = serde_json::from_str(r#"{"last_sent":"2026-11-11"}"#).unwrap();
        assert_eq!(s.period_end(0, 18), Some(at(-6)));
        assert_eq!(s.period_end(0, 99), Some(at(-1)));
        // Same-day legacy record: nothing more today.
        let today: State = serde_json::from_str(r#"{"last_sent":"2026-11-12"}"#).unwrap();
        assert_eq!(next_due(at(20), 0, 18, today.period_end(0, 18)), None);
        // The new field wins; junk or empty means never.
        let both = State {
            period_end_ms: 42,
            last_sent: "2026-11-11".into(),
        };
        assert_eq!(both.period_end(0, 18), Some(42));
        assert_eq!(State::default().period_end(0, 18), None);
        let junk: State = serde_json::from_str(r#"{"last_sent":"garbage"}"#).unwrap();
        assert_eq!(junk.period_end(0, 18), None);
    }

    #[test]
    fn the_local_stamp_and_unavailable_tokens_show_in_the_event() {
        // 23:30 UTC on the 12th is 08:30 on the 13th at +09:00.
        assert_eq!(local_stamp(at(23) + 30 * 60_000, 540), "2026-11-13 08:30");
        let d = Digest {
            from: "2026-11-11 18:00".into(),
            to: "2026-11-12 18:00".into(),
            turns: 1,
            sessions: 1,
            tokens_unavailable: true,
            ..Digest::default()
        };
        let ev = to_event(&d, at(18), false);
        assert_eq!(ev.headline, "2026-11-11 18:00 to 2026-11-12 18:00");
        assert!(ev
            .bullets
            .iter()
            .any(|b| b == "Tokens: totals unavailable (indexing history)"));
    }

    fn rec(ts: i64, k: &str, ws: u32, agent: &str, session: &str, data: Value) -> Record {
        let mut r = Record::new(u64::try_from(ts).unwrap(), k, data);
        r.ws = ws;
        r.agent = agent.into();
        r.session = session.into();
        r
    }

    fn names() -> (impl Fn(u32) -> String, impl Fn(&str) -> String) {
        (
            |ws| match ws {
                1 => "api".to_string(),
                2 => "web".to_string(),
                _ => String::new(),
            },
            |id: &str| {
                if id.is_empty() {
                    "default".into()
                } else {
                    id.to_string()
                }
            },
        )
    }

    fn turn(ts: i64, ws: u32, session: &str, data: Value) -> Record {
        rec(ts, kind::TURN_END, ws, "claude", session, data)
    }

    #[test]
    fn build_counts_the_local_day_only() {
        let (w, a) = names();
        let n = Names {
            workspace: &w,
            account: &a,
        };
        let range = (at(0), at(24));
        let stats = json!({"files": 2, "insertions": 10, "deletions": 3});
        let recs = vec![
            turn(at(-1), 1, "old", json!({"tool_count": 99})),
            turn(
                at(9),
                1,
                "s1",
                json!({"tool_count": 4, "stats": stats, "outcome": {"status": "completed"}}),
            ),
            turn(
                at(10),
                2,
                "s2",
                json!({"tool_count": 1, "outcome": {"status": "failed", "error": "x"}}),
            ),
            turn(at(11), 1, "s1", json!({"tool_count": 0})),
            rec(
                at(12),
                kind::TOOL,
                1,
                "claude",
                "s1",
                json!({"exit_code": 1}),
            ),
            rec(
                at(12),
                kind::TOOL,
                1,
                "claude",
                "s1",
                json!({"exit_code": 0}),
            ),
            rec(
                at(12),
                kind::TOOL,
                1,
                "claude",
                "s1",
                json!({"exit_code": null}),
            ),
            rec(
                at(13),
                kind::PROMPT,
                1,
                "claude",
                "s1",
                json!({"text": "SECRET PROMPT"}),
            ),
            turn(at(24), 1, "next", json!({})),
        ];
        let d = build("2026-11-12", true, range, &recs, &[], &n);
        assert_eq!(d.turns, 3);
        assert_eq!(d.failed_turns, 1);
        assert_eq!(d.sessions, 2);
        assert_eq!(d.tool_calls, 5);
        assert_eq!(d.tool_failures, 1);
        assert_eq!((d.files_changed, d.insertions, d.deletions), (2, 10, 3));
        assert_eq!(
            d.projects[0],
            Counted {
                name: "api".into(),
                count: 2
            }
        );
        assert_eq!(
            d.agents,
            vec![Counted {
                name: "claude".into(),
                count: 3
            }]
        );
        assert!(!d.is_empty());
    }

    #[test]
    fn a_day_with_no_turns_or_tools_is_empty() {
        let (w, a) = names();
        let n = Names {
            workspace: &w,
            account: &a,
        };
        let recs = vec![rec(at(5), kind::PROMPT, 1, "claude", "s", json!({}))];
        let d = build("2026-11-12", false, (at(0), at(24)), &recs, &[], &n);
        assert!(d.is_empty());
        // Tokens alone (other tools on this machine) do not make a day.
        let hours = [HourTokens {
            hour: u64::try_from(at(5) / HOUR_MS).unwrap(),
            account: String::new(),
            provider: "claude".into(),
            total: 1000,
        }];
        let d = build("2026-11-12", false, (at(0), at(24)), &[], &hours, &n);
        assert!(d.is_empty());
        assert_eq!(d.tokens_total, 1000);
    }

    #[test]
    fn tokens_are_cut_by_the_local_day_and_summed_per_account_and_provider() {
        let (w, a) = names();
        let n = Names {
            workspace: &w,
            account: &a,
        };
        let h = |hours: i64, acc: &str, p: &str, total: u64| HourTokens {
            hour: u64::try_from((NOV12 + hours * HOUR_MS) / HOUR_MS).unwrap(),
            account: acc.into(),
            provider: p.into(),
            total,
        };
        let hours = [
            h(-1, "", "claude", 5_000),
            h(1, "", "claude", 1_000),
            h(2, "", "claude", 2_000),
            h(2, "work", "codex", 500),
            h(23, "work", "codex", 700),
            h(24, "", "claude", 9_000),
        ];
        let d = build("2026-11-12", false, (at(0), at(24)), &[], &hours, &n);
        assert_eq!(d.tokens_total, 4_200);
        assert_eq!(
            d.tokens[0],
            TokenLine {
                account: "default".into(),
                provider: "claude".into(),
                total: 3_000
            }
        );
        assert_eq!(d.tokens[1].total, 1_200);
    }

    #[test]
    fn names_are_redacted_and_clipped() {
        let w = |_: u32| format!("deploy sk-ant-api03-{} {}", "A".repeat(40), "x".repeat(200));
        let a = |_: &str| String::new();
        let n = Names {
            workspace: &w,
            account: &a,
        };
        let recs = vec![turn(at(5), 1, "s", json!({}))];
        let d = build("2026-11-12", false, (at(0), at(24)), &recs, &[], &n);
        let shown = &d.projects[0].name;
        assert!(!shown.contains("AAAAAAAAAA"), "{shown}");
        assert!(shown.chars().count() <= NAME_MAX + 1);
    }

    #[test]
    fn the_event_carries_numbers_and_no_prompt() {
        let (w, a) = names();
        let n = Names {
            workspace: &w,
            account: &a,
        };
        let recs = vec![
            turn(
                at(9),
                1,
                "s1",
                json!({"tool_count": 2, "prompt": "TOP SECRET", "last_message": "TOP SECRET"}),
            ),
            rec(
                at(9),
                kind::PROMPT,
                1,
                "claude",
                "s1",
                json!({"text": "TOP SECRET"}),
            ),
        ];
        let d = build("2026-11-12", true, (at(0), at(24)), &recs, &[], &n);
        let ev = to_event(&d, at(20), false);
        assert_eq!(ev.kind, "digest");
        assert_eq!(ev.headline, "2026-11-12 (so far)");
        assert!(ev.prompt.is_none());
        let wire = serde_json::to_string(&ev).unwrap();
        assert!(!wire.contains("TOP SECRET"));
        assert!(ev.bullets[0].starts_with("1 turn in 1 session"));
        assert!(ev.bullets.len() <= 8);
        let test = to_event(&d, at(20), true);
        assert!(test.headline.starts_with("[test send]"));
    }

    #[test]
    fn state_round_trips_and_tolerates_junk() {
        let s = State {
            period_end_ms: 1_794_500_000_000,
            ..State::default()
        };
        let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            serde_json::from_str::<State>("{}").unwrap(),
            State::default()
        );
        assert!(serde_json::from_str::<State>("\"x\"").is_err());
    }

    #[test]
    fn the_local_offset_is_sane() {
        let o = local_offset_minutes(NOV12);
        assert!((-14 * 60..=14 * 60).contains(&o), "{o}");
    }
}
