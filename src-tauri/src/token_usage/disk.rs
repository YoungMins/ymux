use super::parse::Parsed;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};
const MAX_LINE: u64 = 4 * 1024 * 1024;
const MAX_FILES: usize = 20000;
pub(super) fn walk(root: &Path, paths: &mut Vec<PathBuf>, skipped: &mut usize, depth: usize) {
    if fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
        *skipped += 1;
        return;
    }
    if depth > 16 || paths.len() >= MAX_FILES {
        *skipped += 1;
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            *skipped += 1;
            return;
        }
    };
    for entry in entries {
        if paths.len() >= MAX_FILES {
            break;
        }
        let Ok(entry) = entry else {
            *skipped += 1;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            *skipped += 1;
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&entry.path(), paths, skipped, depth + 1);
        } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            paths.push(entry.path());
        }
    }
}

pub(super) fn parse_file(
    path: &Path,
    provider: &'static str,
    fallback: &str,
    charged_bytes: u64,
) -> std::io::Result<Parsed> {
    let mut reader = BufReader::new(fs::File::open(path)?.take(charged_bytes.saturating_add(1)));
    let mut parsed = Parsed::new(fallback);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let count = reader
            .by_ref()
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 {
            break;
        }
        if u64::try_from(count).unwrap_or(u64::MAX) > MAX_LINE {
            parsed.skipped += 1;
            while !bytes.ends_with(b"\n") {
                bytes.clear();
                if reader
                    .by_ref()
                    .take(MAX_LINE)
                    .read_until(b'\n', &mut bytes)?
                    == 0
                {
                    break;
                }
            }
            continue;
        }
        parsed.line(&bytes, provider);
    }
    if reader.into_inner().limit() == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "transcript grew beyond its scan budget",
        ));
    }
    Ok(parsed)
}
