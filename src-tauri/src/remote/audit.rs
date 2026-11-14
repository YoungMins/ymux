//! Append-only audit log of remote commands: `<config>/remote/audit.jsonl`.
//!
//! One JSON object per line: time, device id, op, pane id, decision code.
//! Never prompt text, screen text or labels: every field is either a fixed
//! word or a UUID, and anything else is written as `?`. At 1 MiB the file is
//! renamed to `audit.jsonl.1` (replacing the previous one) and a fresh file
//! starts. `0600` on Unix.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;

pub const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub ts_ms: u64,
    pub device: String,
    /// A fixed word: `list`, `peek`, `answer`, `interrupt`, `prompt`, `pair`,
    /// `unpair`, `cap`, or `?`.
    pub op: &'static str,
    pub pane: String,
    /// `ok`, a protocol code (`bad_sig`, ...), or a short fixed reason.
    pub code: &'static str,
    /// How many rejections this line stands for (1 for an ordinary entry).
    pub count: u32,
}

/// One line per code per this long for rejections that happen before a device
/// is authenticated: the relay can send them at will, and an unbounded stream
/// would push real entries out of the 1 MiB log.
pub const AGG_WINDOW_MS: u64 = 60_000;

/// Open (device, code) windows kept at once. A device id is known only for a
/// paired device (the party was authenticated); the codes are a fixed set, so
/// this is far above what honest use reaches.
const MAX_WINDOWS: usize = 256;

/// Counts pre-authentication rejections per code; [`Aggregate::flush`] turns
/// each finished window into one [`Entry`].
#[derive(Debug, Default)]
pub struct Aggregate {
    /// (device, code, window start, count); the device is empty for a
    /// rejection that happened before one was identified.
    windows: Vec<(String, &'static str, &'static str, u64, u32)>,
}

impl Aggregate {
    pub fn note(&mut self, now_ms: u64, code: &'static str) {
        self.note_device(now_ms, "", code);
    }

    /// Like [`note`](Self::note), but one window per (device, code): for a
    /// rejection a relay can repeat by replaying frames of a known device.
    pub fn note_device(&mut self, now_ms: u64, device: &str, code: &'static str) {
        self.note_op(now_ms, device, "?", code);
    }

    /// One window per (device, op, code). Used for accepted reads (`list`,
    /// `peek`), which a polling phone repeats every couple of seconds: one
    /// line with a count per minute, where a write keeps a line of its own.
    pub fn note_op(&mut self, now_ms: u64, device: &str, op: &'static str, code: &'static str) {
        if let Some(w) = self
            .windows
            .iter_mut()
            .find(|w| w.0 == device && w.1 == op && w.2 == code)
        {
            w.4 = w.4.saturating_add(1);
            return;
        }
        // Many distinct keys at once cannot all get a window of their own;
        // the rest are counted under "no device".
        let device = if self.windows.len() >= MAX_WINDOWS {
            ""
        } else {
            device
        };
        match self
            .windows
            .iter_mut()
            .find(|w| w.0 == device && w.1 == op && w.2 == code)
        {
            Some(w) => w.4 = w.4.saturating_add(1),
            None => self.windows.push((device.to_string(), op, code, now_ms, 1)),
        }
    }

    /// Entries for the windows that are over (all of them with `all`).
    pub fn flush(&mut self, now_ms: u64, all: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        self.windows.retain(|(device, op, code, start, count)| {
            if all || now_ms >= start.saturating_add(AGG_WINDOW_MS) {
                out.push(Entry {
                    ts_ms: *start,
                    device: device.clone(),
                    op,
                    pane: String::new(),
                    code,
                    count: *count,
                });
                false
            } else {
                true
            }
        });
        out
    }
}

/// A UUID as given, else `?` (the field comes off the wire).
fn id_or_unknown(s: &str) -> &str {
    if super::proto::is_uuid(s) {
        s
    } else {
        "?"
    }
}

impl Entry {
    fn line(&self) -> String {
        let pane = if self.pane.is_empty() {
            ""
        } else {
            id_or_unknown(&self.pane)
        };
        // "-" is "no device": the party was not authenticated.
        let device = if self.device.is_empty() {
            "-"
        } else {
            id_or_unknown(&self.device)
        };
        let mut s = json!({
            "t": self.ts_ms,
            "device": device,
            "op": self.op,
            "pane": pane,
            "code": self.code,
            "n": self.count,
        })
        .to_string();
        s.push('\n');
        s
    }
}

pub struct Audit {
    path: PathBuf,
    lock: parking_lot::Mutex<()>,
}

impl Audit {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: parking_lot::Mutex::new(()),
        }
    }

    fn rotated(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".1");
        self.path.with_file_name(name)
    }

    /// Best effort: a full disk must not stop the remote link.
    pub fn record(&self, e: &Entry) {
        let _g = self.lock.lock();
        if let Err(err) = self.append(&e.line()) {
            tracing::warn!(error = %err, "remote audit write failed");
        }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES) {
            let _ = std::fs::remove_file(self.rotated());
            std::fs::rename(&self.path, self.rotated())?;
        }
        let mut f = open_append(&self.path)?;
        f.write_all(line.as_bytes())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "11111111-1111-4111-8111-111111111111";
    const P: &str = "22222222-2222-4222-8222-222222222222";

    fn entry(device: &str, pane: &str) -> Entry {
        Entry {
            ts_ms: 42,
            device: device.into(),
            op: "prompt",
            pane: pane.into(),
            code: "forbidden",
            count: 1,
        }
    }

    #[test]
    fn pre_auth_rejections_become_one_line_per_code_per_minute() {
        let mut a = Aggregate::default();
        for i in 0..1000 {
            a.note(1_000 + i, "bad_sig");
        }
        a.note(2_000, "unknown_device");
        // Nothing yet: the windows are still open.
        assert!(a.flush(30_000, false).is_empty());
        let mut out = a.flush(70_000, false);
        out.sort_by_key(|e| e.code);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].code, out[0].count), ("bad_sig", 1000));
        assert_eq!((out[1].code, out[1].count), ("unknown_device", 1));
        // Flushed windows are gone; a new one starts on the next rejection.
        assert!(a.flush(200_000, false).is_empty());
        a.note(200_000, "bad_sig");
        assert_eq!(a.flush(0, true).len(), 1);
        // The line carries the count and no device or pane.
        let dir = tempfile::tempdir().unwrap();
        let log = Audit::new(dir.path().join("audit.jsonl"));
        log.record(&out[0]);
        let v: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(log.path()).unwrap().trim()).unwrap();
        assert_eq!(v["n"], 1000);
        assert_eq!(v["device"], "-");
    }

    #[test]
    fn distinct_keys_cannot_grow_the_windows_without_bound() {
        let mut a = Aggregate::default();
        for i in 0..5_000 {
            a.note_device(
                1_000,
                &format!("00000000-0000-4000-8000-{i:012}"),
                "forbidden",
            );
        }
        let out = a.flush(0, true);
        assert!(out.len() <= MAX_WINDOWS + 1, "{}", out.len());
        // Nothing was lost: the overflow is counted under no device.
        assert_eq!(out.iter().map(|e| u64::from(e.count)).sum::<u64>(), 5_000);
        assert!(out.iter().any(|e| e.device.is_empty() && e.count > 1));
    }

    #[test]
    fn per_device_windows_are_one_line_per_device_and_code() {
        let mut a = Aggregate::default();
        for i in 0..500 {
            a.note_device(1_000 + i, D, "replay");
            a.note_device(1_000 + i, P, "replay");
        }
        a.note_device(1_500, D, "bad_session");
        assert!(a.flush(30_000, false).is_empty());
        let mut out = a.flush(70_000, false);
        out.sort_by(|x, y| (x.device.as_str(), x.code).cmp(&(y.device.as_str(), y.code)));
        let got: Vec<_> = out
            .iter()
            .map(|e| (e.device.as_str(), e.code, e.count))
            .collect();
        assert_eq!(
            got,
            vec![
                (D, "bad_session", 1),
                (D, "replay", 500),
                (P, "replay", 500)
            ]
        );
        assert!(a.flush(0, true).is_empty());
    }

    #[test]
    fn writes_one_json_line_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let a = Audit::new(dir.path().join("remote").join("audit.jsonl"));
        a.record(&entry(D, P));
        a.record(&entry(D, ""));
        let text = std::fs::read_to_string(a.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["t"], 42);
        assert_eq!(v["device"], D);
        assert_eq!(v["op"], "prompt");
        assert_eq!(v["pane"], P);
        assert_eq!(v["code"], "forbidden");
        let v: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v["pane"], "");
    }

    #[test]
    fn free_text_in_id_fields_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let a = Audit::new(dir.path().join("audit.jsonl"));
        a.record(&entry(
            "rm -rf / and a password hunter2",
            "send this secret",
        ));
        let text = std::fs::read_to_string(a.path()).unwrap();
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("secret"));
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["device"], "?");
        assert_eq!(v["pane"], "?");
    }

    #[test]
    fn rotates_to_one_old_file_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let a = Audit::new(path.clone());
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();
        a.record(&entry(D, P));
        let old = dir.path().join("audit.jsonl.1");
        assert_eq!(std::fs::metadata(&old).unwrap().len(), MAX_BYTES);
        let fresh = std::fs::read_to_string(&path).unwrap();
        assert_eq!(fresh.lines().count(), 1);
        // A second rotation replaces the old copy rather than adding files.
        std::fs::write(&path, vec![b'y'; MAX_BYTES as usize]).unwrap();
        a.record(&entry(D, P));
        assert!(std::fs::read(&old).unwrap().starts_with(b"yyyy"));
        let files = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(files, 2);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let a = Audit::new(dir.path().join("audit.jsonl"));
        a.record(&entry(D, P));
        let mode = std::fs::metadata(a.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
