//! Incremental reader of Codex rollout files.
//!
//! Codex has no hooks ymux can install, so a bound session's rollout JSONL is
//! tailed instead, once per agent-scan tick. A file is followed from its
//! *current end* (history from before ymux looked is not replayed), at most
//! [`MAX_READ`] bytes per poll, and only complete lines are consumed: a
//! half-written last line waits for the next tick. A line longer than the
//! window is skipped whole. Parsing is `activity::codex::parse_line`; the
//! caller supplies which session a pane is bound to. The session id alone is
//! not trusted (a disk-inferred binding can name another directory's
//! conversation), so the first time a rollout is found its `session_meta.cwd`
//! must be the pane's cwd (`ypath::same_path`, CLAUDE.md rule 15); a rollout
//! that cannot be matched is not followed and is looked at again later.

use super::codex::{parse_line, CodexItem};
use crate::agent_scan_disk::{codex_rollout_path, parse_codex_meta, MAX_HEAD_BYTES};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Bytes read per pane per poll.
pub const MAX_READ: u64 = 1024 * 1024;
/// Seconds before a rollout that was not found is searched for again.
const RETRY_SECS: u64 = 10;

#[derive(Debug)]
struct Tracked {
    session: String,
    path: PathBuf,
    offset: u64,
    /// Inside a line longer than [`MAX_READ`]: drop up to its newline.
    skipping: bool,
}

/// What is known about one pane. Owned, so the caller can take it out of the
/// shared [`CodexTailer`], do the disk reads without holding any lock, and
/// put it back.
#[derive(Debug, Default)]
pub struct PaneTail {
    tracked: Option<Tracked>,
    /// `(session, retry at)` for a rollout that was not found or not matched.
    missing: Option<(String, u64)>,
}

impl PaneTail {
    /// New items for the pane's Codex `session`, searching under the Codex
    /// `sessions_root` the first time and checking the rollout belongs to
    /// `pane_cwd`. Never panics, never blocks beyond a few bounded reads.
    pub fn poll(
        &mut self,
        session: &str,
        sessions_root: &Path,
        pane_cwd: &str,
        now_secs: u64,
    ) -> Vec<CodexItem> {
        if self.tracked.as_ref().map_or(true, |t| t.session != session) {
            self.tracked = None;
            if self
                .missing
                .as_ref()
                .is_some_and(|(s, at)| s == session && now_secs < *at)
            {
                return Vec::new();
            }
            let found =
                codex_rollout_path(sessions_root, session).filter(|p| rollout_is_in(p, pane_cwd));
            let Some(path) = found else {
                self.missing = Some((session.to_owned(), now_secs + RETRY_SECS));
                return Vec::new();
            };
            self.missing = None;
            // From the end: only what happens from now on.
            let offset = std::fs::metadata(&path).map_or(0, |m| m.len());
            self.tracked = Some(Tracked {
                session: session.to_owned(),
                path,
                offset,
                skipping: false,
            });
            return Vec::new();
        }
        let Some(t) = self.tracked.as_mut() else {
            return Vec::new();
        };
        read_new(&t.path, &mut t.offset, &mut t.skipping)
            .iter()
            .filter_map(|l| parse_line(l))
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct CodexTailer {
    panes: HashMap<Uuid, PaneTail>,
}

impl CodexTailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Panes currently followed.
    pub fn len(&self) -> usize {
        self.panes.values().filter(|p| p.tracked.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Stop following panes that are not in `live`.
    pub fn retain(&mut self, live: &HashSet<Uuid>) {
        self.panes.retain(|p, _| live.contains(p));
    }

    pub fn forget(&mut self, pane: Uuid) {
        self.panes.remove(&pane);
    }

    /// Takes a pane's state out so its reads can run without the caller
    /// holding the tailer's lock. Pair with [`CodexTailer::put`].
    pub fn take(&mut self, pane: Uuid) -> PaneTail {
        self.panes.remove(&pane).unwrap_or_default()
    }

    pub fn put(&mut self, pane: Uuid, state: PaneTail) {
        self.panes.insert(pane, state);
    }

    /// [`PaneTail::poll`] for `pane`, in place (tests and single-threaded
    /// callers; the app uses `take`/`put` around the reads).
    pub fn poll(
        &mut self,
        pane: Uuid,
        session: &str,
        sessions_root: &Path,
        pane_cwd: &str,
        now_secs: u64,
    ) -> Vec<CodexItem> {
        let mut st = self.take(pane);
        let out = st.poll(session, sessions_root, pane_cwd, now_secs);
        self.put(pane, st);
        out
    }
}

/// Does the rollout's `session_meta` say it ran in `pane_cwd`? An unknown pane
/// cwd, an unreadable file or a missing/odd first line all answer no.
fn rollout_is_in(path: &Path, pane_cwd: &str) -> bool {
    if pane_cwd.trim().is_empty() {
        return false;
    }
    let Ok(f) = File::open(path) else {
        return false;
    };
    let mut line = String::new();
    let mut r = BufReader::new(f.take(MAX_HEAD_BYTES as u64));
    if r.read_line(&mut line).is_err() || !line.ends_with('\n') {
        return false;
    }
    parse_codex_meta(&line).is_some_and(|m| ypath::same_path(&m.cwd, pane_cwd))
}

/// Complete lines appended after `offset` (advancing it). A shrunk file
/// restarts at its new end.
fn read_new(path: &Path, offset: &mut u64, skipping: &mut bool) -> Vec<Vec<u8>> {
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else {
        return Vec::new();
    };
    if len < *offset {
        *offset = len;
        *skipping = false;
        return Vec::new();
    }
    if len == *offset {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let read = File::open(path).and_then(|mut f| {
        f.seek(SeekFrom::Start(*offset))?;
        f.take(MAX_READ).read_to_end(&mut buf)
    });
    if read.is_err() {
        return Vec::new();
    }
    let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n') else {
        if buf.len() as u64 >= MAX_READ {
            // One line larger than the whole window: skip it.
            *offset += buf.len() as u64;
            *skipping = true;
        }
        return Vec::new();
    };
    *offset += (last_nl + 1) as u64;
    let mut lines: Vec<Vec<u8>> = buf[..last_nl]
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    if std::mem::take(skipping) && !lines.is_empty() {
        lines.remove(0);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SID: &str = "01a00000-0000-0000-0000-0000000000aa";

    fn rollout(root: &Path, sid: &str) -> PathBuf {
        let dir = root.join("2026/10/16");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("rollout-2026-10-16T10-00-00-{sid}.jsonl"))
    }

    const CWD: &str = "/work/proj";

    fn meta_line(cwd: &str) -> String {
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SID}\",\"cwd\":\"{cwd}\",\"source\":\"cli\"}}}}\n"
        )
    }

    /// Appends to a rollout, writing its `session_meta` first when new.
    fn append(path: &Path, text: &str) {
        if !path.exists() {
            std::fs::write(path, meta_line(CWD)).unwrap();
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    const START: &str =
        r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1","started_at":1}}"#;
    const DONE: &str =
        r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","duration_ms":5}}"#;

    #[test]
    fn follows_from_the_end_and_waits_for_complete_lines() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        append(&path, &format!("{START}\n"));
        let pane = Uuid::from_u128(1);
        let mut t = CodexTailer::new();
        // First sight: locate, start at EOF, replay nothing.
        assert!(t.poll(pane, SID, root.path(), CWD, 100).is_empty());
        assert_eq!(t.len(), 1);
        assert!(t.poll(pane, SID, root.path(), CWD, 102).is_empty());
        // A half line waits; the rest of it completes it.
        append(&path, &DONE[..20]);
        assert!(t.poll(pane, SID, root.path(), CWD, 104).is_empty());
        append(&path, &format!("{}\n", &DONE[20..]));
        let items = t.poll(pane, SID, root.path(), CWD, 106);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], CodexItem::TaskComplete { .. }));
        assert!(t.poll(pane, SID, root.path(), CWD, 108).is_empty());
        // Garbage and unrecorded lines are skipped, order is kept.
        append(
            &path,
            &format!("not json\n{START}\n{{\"type\":\"other\"}}\n{DONE}\n"),
        );
        let items = t.poll(pane, SID, root.path(), CWD, 110);
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], CodexItem::TaskStarted { .. }));
    }

    #[test]
    fn a_missing_rollout_is_retried_after_the_delay_only() {
        let root = tempfile::tempdir().unwrap();
        let pane = Uuid::from_u128(2);
        let mut t = CodexTailer::new();
        assert!(t.poll(pane, SID, root.path(), CWD, 100).is_empty());
        assert!(t.is_empty());
        // The file appears but we are still inside the retry window.
        let path = rollout(root.path(), SID);
        append(&path, &format!("{START}\n"));
        assert!(t.poll(pane, SID, root.path(), CWD, 105).is_empty());
        assert!(t.is_empty());
        assert!(t.poll(pane, SID, root.path(), CWD, 111).is_empty());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn a_new_session_restarts_at_the_end_of_the_new_file() {
        let root = tempfile::tempdir().unwrap();
        let other = "01a00000-0000-0000-0000-0000000000bb";
        let (p1, p2) = (rollout(root.path(), SID), rollout(root.path(), other));
        append(&p1, &format!("{START}\n"));
        append(&p2, &format!("{START}\n{DONE}\n"));
        let pane = Uuid::from_u128(3);
        let mut t = CodexTailer::new();
        t.poll(pane, SID, root.path(), CWD, 1);
        t.poll(pane, other, root.path(), CWD, 2);
        // Nothing replayed from the second file's history.
        assert!(t.poll(pane, other, root.path(), CWD, 3).is_empty());
        append(&p2, &format!("{START}\n"));
        assert_eq!(t.poll(pane, other, root.path(), CWD, 4).len(), 1);
    }

    #[test]
    fn a_shrunk_file_restarts_and_forget_drops_the_pane() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        append(&path, &format!("{START}\n{START}\n"));
        let pane = Uuid::from_u128(4);
        let mut t = CodexTailer::new();
        t.poll(pane, SID, root.path(), CWD, 1);
        std::fs::write(&path, format!("{START}\n")).unwrap();
        assert!(t.poll(pane, SID, root.path(), CWD, 2).is_empty());
        append(&path, &format!("{DONE}\n"));
        assert_eq!(t.poll(pane, SID, root.path(), CWD, 3).len(), 1);
        t.retain(&HashSet::new());
        assert!(t.is_empty());
        t.poll(pane, SID, root.path(), CWD, 4);
        t.forget(pane);
        assert!(t.is_empty());
    }

    #[test]
    fn reads_at_most_the_window_and_skips_an_oversized_line() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        append(&path, "");
        let pane = Uuid::from_u128(5);
        let mut t = CodexTailer::new();
        t.poll(pane, SID, root.path(), CWD, 1);
        // A 1.5 MiB line, then a good one.
        let big = "x".repeat(MAX_READ as usize + MAX_READ as usize / 2);
        append(&path, &format!("{big}\n{START}\n"));
        let mut got = Vec::new();
        for i in 0..4 {
            got.extend(t.poll(pane, SID, root.path(), CWD, 2 + i));
        }
        assert_eq!(got.len(), 1, "the oversized line is dropped, the next kept");
        assert!(matches!(got[0], CodexItem::TaskStarted { .. }));
    }

    #[test]
    fn an_invalid_session_id_is_never_searched() {
        let root = tempfile::tempdir().unwrap();
        let mut t = CodexTailer::new();
        assert!(t
            .poll(Uuid::from_u128(6), "../../x", root.path(), CWD, 1)
            .is_empty());
        assert!(t.is_empty());
    }

    #[test]
    fn a_rollout_for_another_directory_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        std::fs::write(&path, meta_line("/somewhere/else")).unwrap();
        let pane = Uuid::from_u128(7);
        let mut t = CodexTailer::new();
        assert!(t.poll(pane, SID, root.path(), CWD, 100).is_empty());
        assert!(t.is_empty());
        append(&path, &format!("{START}\n{DONE}\n"));
        // Still inside the retry window, then past it: never followed.
        assert!(t.poll(pane, SID, root.path(), CWD, 105).is_empty());
        assert!(t.poll(pane, SID, root.path(), CWD, 111).is_empty());
        assert!(t.is_empty());
        // The pane's cwd later turns out to match (it was stale).
        assert!(t
            .poll(pane, SID, root.path(), "/somewhere/else", 125)
            .is_empty());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn an_unknown_cwd_or_a_rollout_without_session_meta_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        append(&path, &format!("{START}\n"));
        let pane = Uuid::from_u128(8);
        let mut t = CodexTailer::new();
        assert!(t.poll(pane, SID, root.path(), "", 100).is_empty());
        assert!(t.poll(pane, SID, root.path(), "  ", 120).is_empty());
        assert!(t.is_empty());
        // No session_meta first line at all.
        std::fs::write(&path, format!("{START}\n")).unwrap();
        assert!(t.poll(pane, SID, root.path(), CWD, 140).is_empty());
        assert!(t.is_empty());
        // A first line that is not complete yet is not trusted either.
        std::fs::write(&path, &meta_line(CWD)[..20]).unwrap();
        assert!(t.poll(pane, SID, root.path(), CWD, 160).is_empty());
        assert!(t.is_empty());
    }

    #[test]
    fn the_cwd_match_uses_path_syntax_not_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        std::fs::write(&path, meta_line("C:\\\\Git\\\\Proj")).unwrap();
        let mut t = CodexTailer::new();
        assert!(t
            .poll(Uuid::from_u128(9), SID, root.path(), "c:/git/proj", 1)
            .is_empty());
        assert_eq!(t.len(), 1, "drive-letter paths fold case and separators");
    }

    #[test]
    fn take_and_put_move_the_state_out_from_under_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = rollout(root.path(), SID);
        append(&path, &format!("{START}\n"));
        let pane = Uuid::from_u128(10);
        let mut t = CodexTailer::new();
        let mut st = t.take(pane);
        assert!(t.is_empty());
        assert!(st.poll(SID, root.path(), CWD, 1).is_empty());
        // While the state is out, the tailer can be retained/forgotten freely.
        t.retain(&HashSet::new());
        t.put(pane, st);
        assert_eq!(t.len(), 1);
        append(&path, &format!("{DONE}\n"));
        assert_eq!(t.poll(pane, SID, root.path(), CWD, 2).len(), 1);
    }
}
