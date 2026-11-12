//! Directory walking and the streaming, resumable transcript reader.
use super::parse::Parsed;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

/// A single JSONL line longer than this is skipped (and counted), never held.
const MAX_LINE: u64 = 4 * 1024 * 1024;
/// Safety valve of a listing (see [`list_folders`]); far above any real tree.
pub(super) const MAX_FILES: usize = 1_000_000;
/// How much of a file's start and of the bytes before a resume point are
/// hashed to notice a rewrite that merely made the file longer.
const FINGERPRINT_WINDOW: u64 = 4096;

/// What `(path, len, mtime)` the index keys a transcript by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stat {
    pub len: u64,
    /// Nanoseconds since the epoch; `None` when the platform has no mtime.
    pub mtime: Option<u64>,
}

impl Stat {
    pub fn of(meta: &fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .and_then(|d| u64::try_from(d.as_nanos()).ok()),
        }
    }

    pub fn mtime_secs(&self) -> u64 {
        self.mtime.map_or(0, |n| n / 1_000_000_000)
    }
}

/// How much work one refresh may do: bytes read, and optionally wall time.
pub(super) struct Budget {
    bytes: u64,
    deadline: Option<Instant>,
}

impl Budget {
    pub fn new(bytes: u64, time: Option<Duration>) -> Self {
        Self {
            bytes,
            deadline: time.map(|t| Instant::now() + t),
        }
    }

    pub fn bytes(bytes: u64) -> Self {
        Self::new(bytes, None)
    }

    #[cfg(test)]
    pub fn unlimited() -> Self {
        Self::new(u64::MAX, None)
    }

    pub fn spent(&self) -> bool {
        self.bytes == 0 || self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn charge(&mut self, n: u64) {
        self.bytes = self.bytes.saturating_sub(n);
    }
}

/// What a listing found, and how far it can be trusted. A listing that is not
/// `complete` proves nothing about files it does not mention.
#[derive(Default)]
pub(super) struct Listing {
    pub files: Vec<(PathBuf, Stat)>,
    /// Entries and folders that could not be looked at (counted for the card).
    pub skipped: usize,
    /// Some folder or entry could not be read, or the walk was cut short.
    pub incomplete: bool,
    /// The walk stopped at `limit` files.
    pub truncated: bool,
}

/// Every `*.jsonl` under each of `folders` with the metadata the directory
/// listing already carries (no extra syscall on Windows). `limit` is a safety
/// valve against a runaway tree, not a feature: hitting it sets `truncated`.
pub(super) fn list_folders(folders: &[PathBuf], limit: usize) -> Listing {
    let mut listing = Listing::default();
    for folder in folders {
        walk(folder, &mut listing, 0, limit);
    }
    listing
}

fn walk(root: &Path, out: &mut Listing, depth: usize, limit: usize) {
    if fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
        out.skipped += 1;
        out.incomplete = true;
        return;
    }
    if depth > 16 || out.files.len() >= limit {
        out.skipped += 1;
        out.incomplete = true;
        out.truncated |= out.files.len() >= limit;
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        // Nothing there (yet, or any more): an empty listing, whose absences
        // are confirmed per file by `presence` before anything acts on them.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            out.skipped += 1;
            out.incomplete = true;
            return;
        }
    };
    for entry in entries {
        if out.files.len() >= limit {
            out.truncated = true;
            out.incomplete = true;
            break;
        }
        let Ok(entry) = entry else {
            out.skipped += 1;
            out.incomplete = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            out.skipped += 1;
            out.incomplete = true;
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&entry.path(), out, depth + 1, limit);
        } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            match entry.metadata() {
                Ok(meta) => out.files.push((entry.path(), Stat::of(&meta))),
                Err(_) => {
                    out.skipped += 1;
                    out.incomplete = true;
                }
            }
        }
    }
}

/// Whether a file that a listing did not show is really gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    Present,
    /// Confirmed deleted: the file is `NotFound` while its root is reachable.
    Gone,
    /// Cannot tell (permission, I/O error, a root on a missing drive or an
    /// unavailable share). Treat as still there.
    Unknown,
}

/// Probe one file. `NotFound` alone is not proof: Windows reports a missing
/// drive or share as `NotFound` too, so the file counts as gone only if its
/// root directory is there (or was itself deleted: it is `NotFound` while its
/// parent exists). A root whose parent is missing as well is unavailable.
pub(super) fn presence(path: &Path, root: &Path) -> Presence {
    match fs::symlink_metadata(path) {
        Ok(_) => Presence::Present,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match fs::symlink_metadata(root) {
            Ok(_) => Presence::Gone,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if root
                    .parent()
                    .is_some_and(|parent| fs::symlink_metadata(parent).is_ok())
                {
                    Presence::Gone
                } else {
                    Presence::Unknown
                }
            }
            Err(_) => Presence::Unknown,
        },
        Err(_) => Presence::Unknown,
    }
}

/// Where a pass stopped. `offset` is the start of the first line not yet
/// consumed (a half-written last line stays unconsumed); `scanned` is how far
/// the pass read, which is `end` unless the budget ran out first.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pass {
    pub offset: u64,
    pub scanned: u64,
    /// Bytes read from disk (consumed lines plus a re-read partial tail).
    pub read: u64,
}

fn is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_whitespace)
}

/// Feed the complete lines of `file[start..end]` to `parsed`, calling
/// `after_line` once per line so the caller can drain what it produced.
/// Reads at most `end - start` bytes however much the file grows meanwhile,
/// holds one line at a time, and stops early at a line boundary when
/// `budget` runs out.
pub(super) fn scan(
    file: &mut fs::File,
    start: u64,
    end: u64,
    provider: &'static str,
    parsed: &mut Parsed,
    budget: &mut Budget,
    mut after_line: impl FnMut(&mut Parsed),
) -> std::io::Result<Pass> {
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file.take(end.saturating_sub(start)));
    let mut pos = start;
    let mut read = 0u64;
    let mut buf = Vec::new();
    let mut rest = Vec::new();
    loop {
        if pos > start && budget.spent() {
            return Ok(Pass {
                offset: pos,
                scanned: pos,
                read,
            });
        }
        buf.clear();
        let count = reader
            .by_ref()
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut buf)?;
        if count == 0 {
            break;
        }
        let oversize = count as u64 > MAX_LINE;
        let mut total = count as u64;
        let mut complete = buf.ends_with(b"\n");
        while oversize && !complete {
            rest.clear();
            let more = reader
                .by_ref()
                .take(MAX_LINE)
                .read_until(b'\n', &mut rest)?;
            if more == 0 {
                break;
            }
            total += more as u64;
            complete = rest.ends_with(b"\n");
        }
        read += total;
        if !complete {
            // The file ends mid-line. Valid JSON is a whole line that just
            // lacks its newline; anything else is still being written.
            if oversize
                || (!is_blank(&buf)
                    && serde_json::from_slice::<serde::de::IgnoredAny>(&buf).is_err())
            {
                break;
            }
        }
        if oversize {
            parsed.skipped += 1;
        } else if !is_blank(&buf) {
            parsed.line(&buf, provider);
            after_line(parsed);
        }
        pos += total;
        budget.charge(total);
    }
    Ok(Pass {
        offset: pos,
        scanned: end,
        read,
    })
}

fn fnv(bytes: &[u8], mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) const FNV_SEED: u64 = 0xcbf2_9ce4_8422_2325;

pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    fnv(bytes, FNV_SEED)
}

/// A hash of the file's first bytes and of the bytes just before `offset`:
/// equal to the saved one only if the consumed prefix still looks the same.
pub(super) fn fingerprint(file: &mut fs::File, offset: u64) -> std::io::Result<u64> {
    if offset == 0 {
        return Ok(0);
    }
    let window = offset.min(FINGERPRINT_WINDOW);
    let mut hash = FNV_SEED;
    let mut buf = vec![0u8; usize::try_from(window).unwrap_or(0)];
    for from in [0, offset - window] {
        file.seek(SeekFrom::Start(from))?;
        file.read_exact(&mut buf)?;
        hash = fnv(&buf, hash);
    }
    Ok(hash)
}
