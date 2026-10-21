//! Persisted per-destination retry queue: one JSONL file per destination in an
//! injected directory. Bounded (200 items / 5 MiB, oldest dropped). Appends
//! are single `write` + fsync; rewrites go through a temp file and rename, so
//! a crash leaves either the old or the new file. Unparseable lines (a torn
//! last append) are skipped on read and never abort the queue.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MAX_ITEMS: usize = 200;
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

/// One undelivered message. `payload` is opaque to the queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: String,
    /// Unix ms when enqueued.
    pub ts_ms: i64,
    /// Failed delivery tries so far.
    #[serde(default)]
    pub attempts: u32,
    /// Unix ms before which no retry is due.
    #[serde(default)]
    pub not_before_ms: i64,
    pub payload: serde_json::Value,
}

pub struct Queue {
    path: PathBuf,
    /// Serialises file access and holds what the last full read learned
    /// about due times, so an idle worker's tick does not re-read the file.
    lock: Mutex<NextDue>,
}

/// The earliest `not_before_ms` in the file, as of the last full read. Reset
/// to `Unknown` by every write (push, ack, bump), successful or not, so a
/// stale answer can never hide a due item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextDue {
    Unknown,
    Empty,
    At(i64),
}

fn line_of(item: &QueueItem) -> io::Result<String> {
    let mut s = serde_json::to_string(item).map_err(io::Error::other)?;
    s.push('\n');
    Ok(s)
}

impl Queue {
    /// `<dir>/<dest id>.jsonl`; the directory is created lazily. `None` when
    /// `dest_id` is not a UUID: ids are never sanitized into a file name,
    /// because two different ids could then share one queue.
    pub fn open(dir: &Path, dest_id: &str) -> Option<Queue> {
        super::is_valid_dest_id(dest_id).then(|| Queue {
            path: dir.join(format!("{dest_id}.jsonl")),
            lock: Mutex::new(NextDue::Unknown),
        })
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, NextDue> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The guard, with the due-time cache invalidated: for every write.
    fn write_guard(&self) -> std::sync::MutexGuard<'_, NextDue> {
        let mut g = self.guard();
        *g = NextDue::Unknown;
        g
    }

    fn read_all(&self) -> Vec<QueueItem> {
        let Ok(text) = fs::read(&self.path) else {
            return Vec::new();
        };
        String::from_utf8_lossy(&text)
            .lines()
            .filter_map(|l| serde_json::from_str::<QueueItem>(l).ok())
            .collect()
    }

    fn rewrite(&self, items: &[QueueItem]) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut f = File::create(&tmp)?;
            for it in items {
                f.write_all(line_of(it)?.as_bytes())?;
            }
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
    }

    fn append(&self, item: &QueueItem) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut line = line_of(item)?;
        // A torn previous append has no trailing newline; start a fresh line
        // so the new item is not glued onto garbage.
        if let Ok(mut f) = File::open(&self.path) {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len > 0 {
                use std::io::{Seek, SeekFrom};
                let mut last = [0u8; 1];
                if f.seek(SeekFrom::Start(len - 1)).is_ok()
                    && f.read_exact(&mut last).is_ok()
                    && last[0] != b'\n'
                {
                    line.insert(0, '\n');
                }
            }
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.sync_all()
    }

    /// Adds an item at the back; drops the oldest to stay within bounds.
    /// An item that alone exceeds `MAX_BYTES` is rejected.
    pub fn push(&self, item: &QueueItem) -> io::Result<()> {
        let _g = self.write_guard();
        let size = line_of(item)?.len();
        if size > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "queue item exceeds the queue size cap",
            ));
        }
        let mut items = self.read_all();
        let before = items.len();
        items.retain(|i| i.id != item.id);
        let replaced = items.len() != before;
        items.push(item.clone());
        let mut total: usize = items
            .iter()
            .map(|i| line_of(i).map(|l| l.len()).unwrap_or(0))
            .sum();
        let mut drop_n = items.len().saturating_sub(MAX_ITEMS);
        let mut dropped_bytes = 0usize;
        for it in items.iter().take(drop_n) {
            dropped_bytes += line_of(it).map(|l| l.len()).unwrap_or(0);
        }
        total -= dropped_bytes.min(total);
        while total > MAX_BYTES && drop_n + 1 < items.len() {
            total -= items
                .get(drop_n)
                .and_then(|i| line_of(i).ok())
                .map(|l| l.len())
                .unwrap_or(0)
                .min(total);
            drop_n += 1;
        }
        if drop_n == 0 && !replaced {
            return self.append(item);
        }
        self.rewrite(&items[drop_n..])
    }

    /// Oldest item, if any.
    pub fn peek(&self) -> Option<QueueItem> {
        let _g = self.guard();
        self.read_all().into_iter().next()
    }

    /// All items, oldest first.
    pub fn list(&self) -> Vec<QueueItem> {
        let _g = self.guard();
        self.read_all()
    }

    /// The oldest item due at `now` (`not_before_ms <= now`), if any. While
    /// nothing has been written since the last read and that read found no
    /// item due before some later time, answers from the cache without
    /// touching the file.
    pub fn next_due(&self, now: i64) -> Option<QueueItem> {
        let mut g = self.guard();
        match *g {
            NextDue::Empty => return None,
            NextDue::At(t) if t > now => return None,
            _ => {}
        }
        let items = self.read_all();
        *g = items
            .iter()
            .map(|i| i.not_before_ms)
            .min()
            .map_or(NextDue::Empty, NextDue::At);
        items.into_iter().find(|i| i.not_before_ms <= now)
    }

    pub fn len(&self) -> usize {
        self.list().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes the item (delivered, or dropped for good). Unknown id is ok.
    pub fn ack(&self, id: &str) -> io::Result<()> {
        let _g = self.write_guard();
        let items = self.read_all();
        if !items.iter().any(|i| i.id == id) {
            return Ok(());
        }
        let rest: Vec<QueueItem> = items.into_iter().filter(|i| i.id != id).collect();
        if rest.is_empty() {
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            self.rewrite(&rest)
        }
    }

    /// Records a failed try: attempts += 1 and the next due time. Returns the
    /// new attempt count, or `None` if the id is gone.
    pub fn bump_attempt(&self, id: &str, not_before_ms: i64) -> io::Result<Option<u32>> {
        let _g = self.write_guard();
        let mut items = self.read_all();
        let mut out = None;
        for it in items.iter_mut() {
            if it.id == id {
                it.attempts = it.attempts.saturating_add(1);
                it.not_before_ms = not_before_ms;
                out = Some(it.attempts);
            }
        }
        if out.is_some() {
            self.rewrite(&items)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A destination id; the tests' items have their own short ids.
    const DEST: &str = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";

    fn open(dir: &Path) -> Queue {
        Queue::open(dir, DEST).unwrap()
    }

    fn item(id: &str) -> QueueItem {
        QueueItem {
            id: id.to_string(),
            ts_ms: 1,
            attempts: 0,
            not_before_ms: 0,
            payload: json!({"n": id}),
        }
    }

    #[test]
    fn fifo_push_peek_ack() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        assert!(q.peek().is_none());
        q.push(&item("a")).unwrap();
        q.push(&item("b")).unwrap();
        assert_eq!(q.peek().unwrap().id, "a");
        q.ack("a").unwrap();
        assert_eq!(q.peek().unwrap().id, "b");
        q.ack("b").unwrap();
        assert!(q.is_empty());
        q.ack("missing").unwrap();
    }

    #[test]
    fn survives_reopen() {
        let d = tempfile::tempdir().unwrap();
        open(d.path()).push(&item("a")).unwrap();
        let q = open(d.path());
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().payload, json!({"n": "a"}));
    }

    #[test]
    fn bump_attempt_persists() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        q.push(&item("a")).unwrap();
        assert_eq!(q.bump_attempt("a", 500).unwrap(), Some(1));
        assert_eq!(q.bump_attempt("a", 900).unwrap(), Some(2));
        assert_eq!(q.bump_attempt("zzz", 1).unwrap(), None);
        let it = open(d.path()).peek().unwrap();
        assert_eq!((it.attempts, it.not_before_ms), (2, 900));
    }

    #[test]
    fn drops_oldest_beyond_item_cap() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        for i in 0..(MAX_ITEMS + 5) {
            q.push(&item(&format!("i{i}"))).unwrap();
        }
        let all = q.list();
        assert_eq!(all.len(), MAX_ITEMS);
        assert_eq!(all[0].id, "i5");
        assert_eq!(all.last().unwrap().id, format!("i{}", MAX_ITEMS + 4));
    }

    #[test]
    fn drops_oldest_beyond_byte_cap() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        let big = "z".repeat(MAX_BYTES / 3);
        for i in 0..5 {
            let mut it = item(&format!("b{i}"));
            it.payload = json!({ "blob": big });
            q.push(&it).unwrap();
        }
        let all = q.list();
        assert!(all.len() <= 3 && !all.is_empty());
        assert_eq!(all.last().unwrap().id, "b4");
        let bytes = fs::metadata(d.path().join(format!("{DEST}.jsonl")))
            .unwrap()
            .len() as usize;
        assert!(bytes <= MAX_BYTES);
    }

    #[test]
    fn oversized_single_item_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        let mut it = item("a");
        it.payload = json!("y".repeat(MAX_BYTES + 1));
        assert!(q.push(&it).is_err());
        assert!(q.is_empty());
    }

    #[test]
    fn torn_tail_is_skipped_and_next_push_is_intact() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        q.push(&item("a")).unwrap();
        let p = d.path().join(format!("{DEST}.jsonl"));
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"id\":\"torn\",\"ts_ms\":").unwrap();
        drop(f);
        assert_eq!(q.len(), 1);
        q.push(&item("b")).unwrap();
        let ids: Vec<String> = q.list().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn duplicate_id_replaces() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        q.push(&item("a")).unwrap();
        let mut again = item("a");
        again.payload = json!("new");
        q.push(&again).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().payload, json!("new"));
    }

    #[test]
    fn a_non_uuid_destination_id_is_refused_not_sanitized() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["../../evil/name", "", "x", "a/b", "a_b", "a.b"] {
            assert!(Queue::open(d.path(), bad).is_none(), "{bad:?}");
        }
        // Distinct ids never collapse onto one file.
        let a = Queue::open(d.path(), "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d").unwrap();
        let b = Queue::open(d.path(), "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4e").unwrap();
        a.push(&item("a")).unwrap();
        assert_eq!((a.len(), b.len()), (1, 0));
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[test]
    fn next_due_answers_from_cache_until_a_write() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        let p = d.path().join(format!("{DEST}.jsonl"));
        assert!(q.next_due(0).is_none());
        // Cached as empty: a file written behind the queue's back is not
        // re-read on the next idle tick.
        let mut later = item("a");
        later.not_before_ms = 1_000;
        fs::write(&p, line_of(&later).unwrap()).unwrap();
        assert!(q.next_due(5_000).is_none());
        // Any write invalidates.
        q.push(&item("b")).unwrap();
        assert_eq!(q.next_due(0).unwrap().id, "b");
        q.ack("b").unwrap();
        // Only "a" (due at 1000) is left: cached, so it is not due at 999
        // even after the file is removed underneath...
        assert!(q.next_due(999).is_none());
        fs::remove_file(&p).unwrap();
        assert!(q.next_due(999).is_none());
        // ...and once due the file is read again.
        assert!(q.next_due(1_000).is_none());
        fs::write(&p, line_of(&later).unwrap()).unwrap();
        assert!(
            q.next_due(1_000).is_none(),
            "cached empty after the last read"
        );
        q.bump_attempt("a", 2_000).unwrap();
        assert!(q.next_due(1_999).is_none());
        assert_eq!(q.next_due(2_000).unwrap().attempts, 1);
    }

    #[test]
    fn a_failed_write_still_invalidates_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        assert!(q.next_due(0).is_none());
        // The queue path is a directory: append fails.
        let p = d.path().join(format!("{DEST}.jsonl"));
        fs::create_dir_all(&p).unwrap();
        assert!(q.push(&item("a")).is_err());
        fs::remove_dir(&p).unwrap();
        fs::write(&p, line_of(&item("a")).unwrap()).unwrap();
        assert_eq!(q.next_due(0).unwrap().id, "a");
    }

    #[test]
    fn no_temp_file_left_after_rewrite() {
        let d = tempfile::tempdir().unwrap();
        let q = open(d.path());
        q.push(&item("a")).unwrap();
        q.push(&item("b")).unwrap();
        q.ack("a").unwrap();
        let names: Vec<String> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![format!("{DEST}.jsonl")]);
    }
}
