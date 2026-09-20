//! Dock-mode preview: the head of whatever the cursor is on.
//!
//! The dock is a tall, narrow strip. Two side-by-side listings squeezed
//! into it were the complaint; one listing with a preview under it is the
//! fix (spec §2). Everything here that decides *what* the preview says is
//! pure and takes bytes, so it can be tested without a disk.

use std::path::Path;

/// Rows the listing needs to be worth drawing: two border rows, the column
/// header, and three entries. Fewer than three visible entries and the
/// cursor is effectively scrolling one row at a time through a keyhole.
pub const MIN_LIST_ROWS: u16 = 6;

/// Rows the preview needs to be worth drawing: two border rows and three
/// lines of content. Two lines of a file tell you nothing a filename
/// doesn't, and the border is not optional — without it the preview reads
/// as more listing rows.
pub const MIN_PREVIEW_ROWS: u16 = 5;

/// Below this many rows the dock shows the listing alone. Splitting a
/// 10-row dock would leave a 5-row listing, and the listing is the thing
/// the user came for.
pub const MIN_SPLIT_ROWS: u16 = MIN_LIST_ROWS + MIN_PREVIEW_ROWS;

/// Never read more than this from a file. A preview of a 2 GB log must
/// cost the same as a preview of a README, because the read happens on the
/// event-loop thread between two keystrokes.
pub const MAX_PREVIEW_BYTES: usize = 64 * 1024;

/// Never keep more lines than this, however short they are.
pub const MAX_PREVIEW_LINES: usize = 200;

/// Directory entries the preview names before saying "… and N more".
pub const MAX_PREVIEW_ENTRIES: usize = 200;

/// How far into a directory the preview walks. The same bound as the byte
/// cap, for the same reason: `node_modules` must not stall a cursor move.
pub const MAX_PREVIEW_SCAN: usize = 512;

/// How much of a file's head decides whether it is binary.
///
/// Deliberately smaller than [`MAX_PREVIEW_BYTES`] and shared with
/// `app::is_binary_file`: Enter and the preview must agree about one file,
/// or a NUL at byte 20000 gets "(binary file)" in the dock and then opens
/// happily in ycode.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// A tab is drawn as this many spaces. Rendering a literal `\t` into a
/// ratatui buffer produces a one-cell hole, not a stop.
const TAB_WIDTH: usize = 4;

/// What the preview pane has to say about the selected entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// Nothing is selected (an empty directory).
    Empty,
    /// The head of a text file. Empty when the file is.
    Text(Vec<String>),
    /// The selected directory's own entries, and whether the walk stopped
    /// short of the end.
    Dir { names: Vec<String>, more: bool },
    /// A file with a NUL byte in its head. Printing it would be noise at
    /// best and would repaint the dock at worst.
    Binary,
    /// The file could not be read. Worth showing: a permission error on a
    /// cursor move is otherwise invisible.
    Error(String),
}

/// Split the dock body into (listing rows, preview rows).
///
/// Roughly 60/40 (spec §2), with both halves held above their minimums and
/// the preview dropped outright when both cannot be met.
pub fn split_dock(body: u16) -> (u16, u16) {
    if body < MIN_SPLIT_ROWS {
        return (body, 0);
    }
    let preview = ((u32::from(body) * 40) / 100) as u16;
    let preview = preview.clamp(MIN_PREVIEW_ROWS, body - MIN_LIST_ROWS);
    (body - preview, preview)
}

/// Decode the head of a file into preview lines.
///
/// `chunk` is at most [`MAX_PREVIEW_BYTES`], so it can end in the middle of
/// a character — dropping that stub is what keeps a Korean file from ending
/// in a replacement glyph. A byte that is invalid *within* the chunk is a
/// different thing (a latin-1 file, say) and is decoded lossily rather than
/// truncating everything after it.
pub fn decode_preview(chunk: &[u8], max_lines: usize) -> Preview {
    if is_binary(chunk) {
        return Preview::Binary;
    }
    let end = match std::str::from_utf8(chunk) {
        Ok(_) => chunk.len(),
        Err(e) => match e.error_len() {
            // Incomplete sequence at the very end: the read cap cut a
            // character in half.
            None => e.valid_up_to(),
            // A genuinely bad byte somewhere inside: keep the rest, lossily.
            Some(_) => chunk.len(),
        },
    };
    let text = String::from_utf8_lossy(&chunk[..end]);
    Preview::Text(text.lines().take(max_lines).map(sanitize).collect())
}

/// Whether the head of a file reads as binary: a NUL byte in the first
/// [`BINARY_SNIFF_BYTES`].
pub fn is_binary(head: &[u8]) -> bool {
    head[..head.len().min(BINARY_SNIFF_BYTES)].contains(&0u8)
}

/// Make one line safe to render: tabs become spaces, and every other
/// control character is dropped. A file full of ANSI escapes must not be
/// able to repaint the dock from inside a preview.
fn sanitize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            '\t' => out.push_str(&" ".repeat(TAB_WIDTH)),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Format a directory walk as preview lines: directories first, then names,
/// matching the listing's own order.
pub fn directory_preview(mut names: Vec<(String, bool)>, more: bool) -> Preview {
    names.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let shown = names.len().min(MAX_PREVIEW_ENTRIES);
    let more = more || names.len() > shown;
    let names = names
        .into_iter()
        .take(shown)
        .map(|(name, is_dir)| if is_dir { format!("{name}/") } else { name })
        .collect();
    Preview::Dir { names, more }
}

/// Build the preview for `path`. The only function here that touches the
/// disk; both bounds above are applied before anything is decoded.
pub fn read_preview(path: &Path, is_dir: bool, show_hidden: bool) -> Preview {
    if is_dir {
        read_dir_preview(path, show_hidden)
    } else {
        read_file_preview(path)
    }
}

fn read_file_preview(path: &Path) -> Preview {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return Preview::Error(e.to_string()),
    };
    let mut buf = vec![0u8; MAX_PREVIEW_BYTES];
    let mut filled = 0usize;
    // One `read` can come back short of the cap without being at EOF.
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) => return Preview::Error(e.to_string()),
        }
    }
    buf.truncate(filled);
    decode_preview(&buf, MAX_PREVIEW_LINES)
}

fn read_dir_preview(path: &Path, show_hidden: bool) -> Preview {
    let iter = match std::fs::read_dir(path) {
        Ok(it) => it,
        Err(e) => return Preview::Error(e.to_string()),
    };
    let mut names = Vec::new();
    let mut more = false;
    // Counts entries walked, not entries kept: a directory of 100k hidden
    // files must cost the same as any other.
    for (scanned, item) in iter.enumerate() {
        if scanned >= MAX_PREVIEW_SCAN {
            more = true;
            break;
        }
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);
        names.push((name, is_dir));
    }
    directory_preview(names, more)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(p: &Preview) -> &[String] {
        match p {
            Preview::Text(l) => l,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn split_is_roughly_sixty_forty() {
        assert_eq!(split_dock(40), (24, 16));
        assert_eq!(split_dock(20), (12, 8));
        assert_eq!(split_dock(50), (30, 20));
    }

    #[test]
    fn split_drops_the_preview_in_a_short_dock() {
        for body in 0..MIN_SPLIT_ROWS {
            assert_eq!(split_dock(body), (body, 0), "body {body}");
        }
    }

    /// At the threshold the split has to hand both halves their minimum,
    /// not the 40% the ratio asks for (which would be 4 rows).
    #[test]
    fn split_honours_both_minimums_at_the_threshold() {
        assert_eq!(
            split_dock(MIN_SPLIT_ROWS),
            (MIN_LIST_ROWS, MIN_PREVIEW_ROWS)
        );
        for body in MIN_SPLIT_ROWS..=200 {
            let (list, preview) = split_dock(body);
            assert_eq!(list + preview, body, "body {body} does not add up");
            assert!(list >= MIN_LIST_ROWS, "body {body} starved the listing");
            assert!(
                preview >= MIN_PREVIEW_ROWS,
                "body {body} starved the preview"
            );
        }
    }

    #[test]
    fn decode_keeps_the_head_of_a_text_file() {
        let p = decode_preview(b"one\ntwo\nthree\n", 2);
        assert_eq!(lines(&p), ["one", "two"]);
    }

    #[test]
    fn decode_strips_carriage_returns_and_expands_tabs() {
        let p = decode_preview(b"a\tb\r\nc\r\n", 10);
        assert_eq!(lines(&p), ["a    b", "c"]);
    }

    /// A preview must never be able to move the cursor or recolour the dock.
    #[test]
    fn decode_drops_escape_sequences() {
        let p = decode_preview(b"\x1b[2Jgone\x07\n", 10);
        assert_eq!(lines(&p), ["[2Jgone"]);
    }

    #[test]
    fn the_binary_sniff_stops_at_its_window() {
        // The same window Enter uses, so the dock and ycode cannot
        // disagree about whether one file is text.
        let mut late = vec![b'x'; BINARY_SNIFF_BYTES];
        late.push(0);
        assert!(!is_binary(&late));
        let mut early = vec![b'x'; BINARY_SNIFF_BYTES - 1];
        early.push(0);
        assert!(is_binary(&early));
        assert!(!is_binary(b""));
    }

    #[test]
    fn decode_calls_a_nul_byte_binary() {
        assert_eq!(decode_preview(b"ELF\0\x01\x02", 10), Preview::Binary);
    }

    #[test]
    fn decode_handles_an_empty_file() {
        assert_eq!(decode_preview(b"", 10), Preview::Text(vec![]));
    }

    /// Korean is the point of the exercise: the user's files are Korean.
    #[test]
    fn decode_reads_hangul_intact() {
        let p = decode_preview("첫 줄입니다\n둘째 줄\n".as_bytes(), 10);
        assert_eq!(lines(&p), ["첫 줄입니다", "둘째 줄"]);
    }

    /// The 64 KiB cap lands wherever it lands, including halfway through a
    /// Hangul syllable. That stub must vanish, not become U+FFFD.
    #[test]
    fn decode_drops_a_character_the_read_cap_cut_in_half() {
        let mut bytes = "안녕하세요".as_bytes().to_vec();
        bytes.truncate(bytes.len() - 1); // last syllable is now 2 of 3 bytes
        let p = decode_preview(&bytes, 10);
        assert_eq!(lines(&p), ["안녕하세"]);
        assert!(!lines(&p)[0].contains('\u{fffd}'));
    }

    /// A one-line minified file has no newline to trim back to, so trimming
    /// by line would leave the preview blank.
    #[test]
    fn decode_keeps_a_file_with_no_newline_at_all() {
        let p = decode_preview(b"{\"a\":1,\"b\":2}", 10);
        assert_eq!(lines(&p), ["{\"a\":1,\"b\":2}"]);
    }

    /// An invalid byte in the middle is a latin-1 file, not a cut character:
    /// everything after it still has to show up.
    #[test]
    fn decode_falls_back_to_lossy_for_a_bad_byte_inside() {
        let p = decode_preview(b"caf\xe9 au lait\nsecond\n", 10);
        assert_eq!(lines(&p).len(), 2);
        assert!(lines(&p)[0].contains("au lait"));
        assert_eq!(lines(&p)[1], "second");
    }

    #[test]
    fn directory_preview_puts_directories_first_and_marks_them() {
        let p = directory_preview(
            vec![
                ("zeta.txt".into(), false),
                ("Alpha".into(), true),
                ("beta.rs".into(), false),
            ],
            false,
        );
        assert_eq!(
            p,
            Preview::Dir {
                names: vec!["Alpha/".into(), "beta.rs".into(), "zeta.txt".into()],
                more: false,
            }
        );
    }

    #[test]
    fn directory_preview_caps_the_entries_it_names() {
        let names: Vec<(String, bool)> = (0..MAX_PREVIEW_ENTRIES + 10)
            .map(|i| (format!("f{i:04}"), false))
            .collect();
        let Preview::Dir { names, more } = directory_preview(names, false) else {
            panic!("expected a directory preview");
        };
        assert_eq!(names.len(), MAX_PREVIEW_ENTRIES);
        assert!(more);
    }

    #[test]
    fn read_preview_reads_a_real_file_and_a_real_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("보고서.txt"), "한 줄\n두 줄\n").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let p = read_preview(&dir.join("보고서.txt"), false, false);
        assert_eq!(lines(&p), ["한 줄", "두 줄"]);

        let Preview::Dir { names, .. } = read_preview(dir, true, false) else {
            panic!("expected a directory preview");
        };
        assert_eq!(names, vec!["sub/".to_string(), "보고서.txt".to_string()]);
    }

    #[test]
    fn read_preview_reports_a_missing_file_instead_of_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_preview(&tmp.path().join("nope"), false, false),
            Preview::Error(_)
        ));
        assert!(matches!(
            read_preview(&tmp.path().join("nope"), true, false),
            Preview::Error(_)
        ));
    }

    /// The cap is on bytes read, so a huge file costs the same as a small
    /// one. Nothing past it may reach the preview.
    #[test]
    fn read_preview_stops_at_the_byte_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.txt");
        let body = "x".repeat(40) + "\n";
        std::fs::write(&path, body.repeat(10_000)).unwrap();
        let p = read_preview(&path, false, false);
        assert_eq!(lines(&p).len(), MAX_PREVIEW_LINES);
    }
}
