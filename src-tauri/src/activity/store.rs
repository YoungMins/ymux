//! Append-only daily JSONL activity log.
//!
//! One file per UTC day, `<dir>/YYYY-MM-DD.jsonl`, one [`Record`] per line.
//! The directory is injected (`<config>/ymux/activity/` in the app), so tests
//! use a temp dir. Caps: a per-day size cap (the noisy `tool` kind is dropped
//! first once a day is full) and a total cap plus age-based retention that
//! [`ActivityStore::prune`] enforces. No function panics.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_DAY_CAP_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_RETENTION_DAYS: u32 = 14;
pub const DEFAULT_TOTAL_CAP_BYTES: u64 = 200 * 1024 * 1024;

/// Record kinds. `TOOL` is the one dropped first when a day is over its cap.
pub mod kind {
    pub const PROMPT: &str = "prompt";
    pub const TOOL: &str = "tool";
    pub const PERMISSION: &str = "permission";
    pub const TURN_END: &str = "turn_end";
    pub const SUMMARY: &str = "summary";
}

/// Schema v1. Every field has a serde default so an older or newer line
/// still parses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub id: String,
    /// Unix milliseconds.
    #[serde(default)]
    pub ts: u64,
    /// Workspace id, from backend state (never a hook claim).
    #[serde(default)]
    pub ws: u32,
    #[serde(default)]
    pub pane: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub turn: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub kind: String,
    /// Where the pane's session id came from (`hook`, `process`, `disk`);
    /// empty when not applicable. A `disk` id is an inference, so its
    /// records stay local (see the worker).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id_source: String,
    #[serde(default)]
    pub data: Value,
}

impl Record {
    /// A v1 record with a fresh id; the caller fills the context fields.
    pub fn new(ts: u64, kind: &str, data: Value) -> Self {
        Self {
            v: SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().simple().to_string(),
            ts,
            ws: 0,
            pane: String::new(),
            agent: String::new(),
            account: String::new(),
            session: String::new(),
            turn: String::new(),
            cwd: String::new(),
            branch: String::new(),
            kind: kind.to_owned(),
            id_source: String::new(),
            data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    Written,
    /// Over the day cap: dropped, not an error.
    Dropped,
}

#[derive(Debug, Clone)]
pub struct ActivityStore {
    dir: PathBuf,
    day_cap: u64,
    total_cap: u64,
    retention_days: u32,
}

impl ActivityStore {
    pub fn new(dir: impl Into<PathBuf>, retention_days: u32, max_total_mb: u32) -> Self {
        Self {
            dir: dir.into(),
            day_cap: DEFAULT_DAY_CAP_BYTES,
            total_cap: u64::from(max_total_mb).saturating_mul(1024 * 1024),
            retention_days,
        }
    }

    /// Override the per-day cap (tests; the app uses the default).
    pub fn with_day_cap(mut self, bytes: u64) -> Self {
        self.day_cap = bytes;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn day_path(&self, day: &str) -> PathBuf {
        self.dir.join(format!("{day}.jsonl"))
    }

    /// Append one record to the file of its UTC day.
    ///
    /// Over the day cap, `tool` records are dropped; every other kind is
    /// still written up to 25% over the cap, then dropped too, so a runaway
    /// day can never grow without bound.
    pub fn append(&self, rec: &Record) -> io::Result<Appended> {
        let mut line = serde_json::to_string(rec).map_err(io::Error::other)?;
        line.push('\n');
        fs::create_dir_all(&self.dir)?;
        let path = self.day_path(&day_string(rec.ts));
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let after = size.saturating_add(line.len() as u64);
        let hard = self.day_cap.saturating_add(self.day_cap / 4);
        if (rec.kind == kind::TOOL && after > self.day_cap) || after > hard {
            return Ok(Appended::Dropped);
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        f.write_all(line.as_bytes())?;
        Ok(Appended::Written)
    }

    /// `(day, path, size)` of every `YYYY-MM-DD.jsonl`, oldest first.
    fn files(&self) -> Vec<(String, PathBuf, u64)> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<_> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let day = name.strip_suffix(".jsonl")?.to_owned();
                day_number(&day)?;
                let len = e.metadata().ok()?.len();
                Some((day, e.path(), len))
            })
            .collect();
        out.sort();
        out
    }

    /// Delete files older than the retention window, then the oldest files
    /// until the total fits the cap (the newest file is never deleted here).
    /// Returns how many files were removed.
    pub fn prune(&self, now_ms: u64) -> io::Result<usize> {
        let today = i64::try_from(now_ms / 86_400_000).unwrap_or(i64::MAX);
        let oldest_kept = today.saturating_sub(i64::from(self.retention_days));
        let mut removed = 0;
        let mut files = self.files();
        files.retain(|(day, path, _)| {
            let expired = day_number(day).is_some_and(|d| d < oldest_kept);
            if expired && fs::remove_file(path).is_ok() {
                removed += 1;
                return false;
            }
            true
        });
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        while total > self.total_cap && files.len() > 1 {
            let (_, path, len) = files.remove(0);
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            total = total.saturating_sub(len);
        }
        Ok(removed)
    }

    /// Delete every daily file. Returns how many were removed.
    pub fn clear(&self) -> usize {
        self.files()
            .into_iter()
            .filter(|(_, path, _)| fs::remove_file(path).is_ok())
            .count()
    }

    /// The newest `n` records, oldest first. Unparseable lines are skipped.
    pub fn read_recent(&self, n: usize) -> Vec<Record> {
        let mut out: Vec<Record> = Vec::new();
        for (_, path, _) in self.files().into_iter().rev() {
            if out.len() >= n {
                break;
            }
            let Ok(f) = File::open(&path) else { continue };
            let mut day: Vec<Record> = BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .filter_map(|l| serde_json::from_str(&l).ok())
                .collect();
            day.extend(out);
            out = day;
            if out.len() > n {
                out.drain(..out.len() - n);
            }
        }
        out
    }
}

/// `YYYY-MM-DD` (UTC) for a Unix-millisecond timestamp.
pub fn day_string(ts_ms: u64) -> String {
    let days = i64::try_from(ts_ms / 86_400_000).unwrap_or(0);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the Unix epoch for a strict `YYYY-MM-DD`, else `None`.
fn day_number(day: &str) -> Option<i64> {
    let b = day.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    // Digits only: `str::parse` would also accept a sign.
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return None;
    }
    let y: i64 = day.get(0..4)?.parse().ok()?;
    let m: i64 = day.get(5..7)?.parse().ok()?;
    let d: i64 = day.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days_from_civil.
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DAY: u64 = 86_400_000;
    // 2026-10-16T00:00:00Z
    const D0: u64 = 1_792_108_800_000;

    fn store(dir: &tempfile::TempDir) -> ActivityStore {
        ActivityStore::new(dir.path().join("activity"), 14, 200)
    }

    fn rec(ts: u64, kind: &str) -> Record {
        let mut r = Record::new(ts, kind, json!({"x": 1}));
        r.pane = "p".into();
        r
    }

    #[test]
    fn day_string_and_number_roundtrip() {
        assert_eq!(day_string(0), "1970-01-01");
        assert_eq!(day_string(D0), "2026-10-16");
        assert_eq!(day_string(D0 + DAY - 1), "2026-10-16");
        assert_eq!(day_string(951_782_400_000), "2000-02-29");
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2026-10-16"), Some((D0 / DAY) as i64));
        assert_eq!(day_number("2026-13-01"), None);
        assert_eq!(day_number("nope"), None);
        assert_eq!(day_number("2026-1-016"), None);
        assert_eq!(day_number("é026-10-16"), None);
    }

    #[test]
    fn append_then_read_recent_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(&tmp);
        for i in 0..5 {
            assert_eq!(
                s.append(&rec(D0 + i, kind::PROMPT)).unwrap(),
                Appended::Written
            );
        }
        s.append(&rec(D0 + DAY, kind::TURN_END)).unwrap();
        assert!(tmp.path().join("activity/2026-10-16.jsonl").exists());
        assert!(tmp.path().join("activity/2026-10-17.jsonl").exists());
        let last3 = s.read_recent(3);
        assert_eq!(
            last3.iter().map(|r| r.ts).collect::<Vec<_>>(),
            vec![D0 + 3, D0 + 4, D0 + DAY]
        );
        assert_eq!(s.read_recent(100).len(), 6);
        assert_eq!(last3[0].v, SCHEMA_VERSION);
        assert_eq!(last3[0].data["x"], 1);
        assert!(s.read_recent(0).is_empty());
    }

    #[test]
    fn read_recent_skips_garbage_and_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(&tmp);
        assert!(s.read_recent(5).is_empty());
        s.append(&rec(D0, kind::PROMPT)).unwrap();
        let p = tmp.path().join("activity/2026-10-16.jsonl");
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{not json\n\n").unwrap();
        s.append(&rec(D0 + 1, kind::PROMPT)).unwrap();
        fs::write(tmp.path().join("activity/notes.txt"), "x").unwrap();
        assert_eq!(s.read_recent(10).len(), 2);
    }

    #[test]
    fn unknown_and_missing_fields_parse() {
        let r: Record = serde_json::from_str(r#"{"v":2,"kind":"tool","future":true}"#).unwrap();
        assert_eq!(r.v, 2);
        assert_eq!(r.kind, "tool");
        assert_eq!(r.ws, 0);
    }

    #[test]
    fn day_cap_drops_tool_records_first() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(&tmp).with_day_cap(1000);
        let mut tools = 0;
        while s.append(&rec(D0, kind::TOOL)).unwrap() == Appended::Written {
            tools += 1;
            assert!(tools < 100);
        }
        let size = |s: &ActivityStore| {
            fs::metadata(s.dir().join("2026-10-16.jsonl"))
                .unwrap()
                .len()
        };
        assert!(size(&s) <= 1000);
        // Non-tool kinds still get through the headroom...
        assert_eq!(
            s.append(&rec(D0, kind::TURN_END)).unwrap(),
            Appended::Written
        );
        // ...until the hard cap (125%).
        while s.append(&rec(D0, kind::TURN_END)).unwrap() == Appended::Written {}
        assert!(size(&s) <= 1250);
        // Another day is unaffected.
        assert_eq!(
            s.append(&rec(D0 + DAY, kind::TOOL)).unwrap(),
            Appended::Written
        );
    }

    #[test]
    fn prune_removes_expired_days_only() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(&tmp);
        for back in [0u64, 13, 14, 15, 40] {
            s.append(&rec(D0 - back * DAY, kind::PROMPT)).unwrap();
        }
        let removed = s.prune(D0 + 1000).unwrap();
        assert_eq!(removed, 2); // 15 and 40 days old
        assert_eq!(s.read_recent(10).len(), 3);
    }

    #[test]
    fn prune_enforces_total_cap_but_keeps_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = store(&tmp);
        s.total_cap = 1; // everything is over
        for back in [2u64, 1, 0] {
            s.append(&rec(D0 - back * DAY, kind::PROMPT)).unwrap();
        }
        assert_eq!(s.prune(D0).unwrap(), 2);
        let left = s.read_recent(10);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].ts, D0);
        assert_eq!(s.prune(D0).unwrap(), 0);
        // Missing dir is not an error.
        let empty = ActivityStore::new(tmp.path().join("nope"), 14, 200);
        assert_eq!(empty.prune(D0).unwrap(), 0);
    }

    #[test]
    fn clear_removes_every_day_and_only_day_files() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(&tmp);
        for back in [2u64, 1, 0] {
            s.append(&rec(D0 - back * DAY, kind::PROMPT)).unwrap();
        }
        std::fs::write(s.dir().join("notes.txt"), "keep").unwrap();
        assert_eq!(s.clear(), 3);
        assert!(s.read_recent(10).is_empty());
        assert!(s.dir().join("notes.txt").exists());
        assert_eq!(s.clear(), 0);
        let empty = ActivityStore::new(tmp.path().join("nope"), 14, 200);
        assert_eq!(empty.clear(), 0);
    }

    #[test]
    fn bad_day_filenames_are_ignored() {
        assert!(day_number("2026-10-16").is_some());
        for bad in [
            "+026-10-16",
            "2026-+1-16",
            "2026-10-+6",
            "-026-10-16",
            "2026-1 -16",
            "2026-13-01",
            "2026-00-10",
            "2026-10-00",
            "2026/10/16",
            "26-10-2016",
            "20261016",
            "",
        ] {
            assert!(day_number(bad).is_none(), "{bad:?}");
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("activity");
        fs::create_dir_all(&dir).unwrap();
        for n in ["+026-10-16.jsonl", "junk.jsonl", "2026-10-16.jsonl.bak"] {
            fs::write(dir.join(n), "{}\n").unwrap();
        }
        let st = ActivityStore::new(&dir, 14, 200);
        assert!(st.files().is_empty());
        assert_eq!(st.prune(1_792_108_800_000).unwrap(), 0);
        assert_eq!(st.clear(), 0);
        assert!(dir.join("junk.jsonl").exists());
    }
}
