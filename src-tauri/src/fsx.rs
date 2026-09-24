//! The decisions a file listing makes, with no filesystem attached.
//!
//! What to show, in what order, how to print a size, whether a file's head
//! reads as binary, and whether two paths are the same one. The syscalls
//! that feed these live in `fsops.rs`, which is `desktop`-gated; everything
//! here is pure and runs under
//! `cargo test --no-default-features --lib -p ymux` on Linux CI, the same
//! split `agents.rs`/`agent_scan.rs` and `agent_hooks.rs` already use
//! (CLAUDE.md rule 1).
//!
//! The order and the hidden rule deliberately match `tools/ydir`'s, so the
//! GUI files pane that replaces it feels the same under the hands.

use serde::{Deserialize, Serialize};

/// One row of a directory listing, as the frontend sees it.
///
/// `size` and `modified_ms` are 0 for an entry whose metadata could not be
/// read — a listing must never fail because one row is locked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntryInfo {
    /// File name only, no directory part.
    pub name: String,
    /// Absolute path. The raw spelling, never a [`ypath`] comparison key.
    pub path: String,
    pub is_dir: bool,
    /// The entry itself is a symlink (or, on Windows, a junction/reparse
    /// point). `is_dir` still describes the *target*, because that is what
    /// decides whether double-clicking navigates.
    pub is_symlink: bool,
    pub size: u64,
    /// Unix-epoch milliseconds, or 0 if unknown.
    pub modified_ms: u64,
}

/// How much of a file's head decides "binary".
///
/// Ported from `tools/ydir/src/preview.rs`'s `BINARY_SNIFF_BYTES`, and kept
/// identical for the reason that file gives: the listing and the preview must
/// agree about one file, or a NUL at byte 20000 shows "(binary file)" in the
/// preview and then opens as text in the editor.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Whether the head of a file reads as binary: a NUL byte in the first
/// [`BINARY_SNIFF_BYTES`].
///
/// Total, and deliberately crude. Invalid UTF-8 alone is *not* binary —
/// that is [`crate::YmuxError::NotUtf8`]'s job, and a legacy CP949 text file
/// should be reported as "not UTF-8", not as "binary".
pub fn is_probably_binary(head: &[u8]) -> bool {
    head[..head.len().min(BINARY_SNIFF_BYTES)].contains(&0u8)
}

/// Is `name` hidden by the dotfile convention?
///
/// `.` and `..` are not entries a listing ever contains, but guarding them
/// keeps the function total for a caller that passes one.
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

/// Drop dotfiles unless the pane is showing them.
///
/// The Windows *hidden attribute* is a second, independent rule; `fsops.rs`
/// folds it into the same pass because it needs a syscall to read.
pub fn apply_hidden(entries: &mut Vec<DirEntryInfo>, show_hidden: bool) {
    if !show_hidden {
        entries.retain(|e| !is_hidden_name(&e.name));
    }
}

/// Directories first, then by name, case-insensitively.
///
/// Byte-identical ordering to `tools/ydir/src/app.rs:490`. `to_lowercase`
/// rather than a locale collation for the same reason ydir chose it: it is
/// the order users have, and changing it is a separate decision from
/// changing the renderer.
pub fn sort_entries(entries: &mut [DirEntryInfo]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Human-readable size, matching `tools/ydir`'s `size_display` exactly
/// (including `<DIR>` for a directory, so the column reads the same).
pub fn format_size(size: u64, is_dir: bool) -> String {
    const K: u64 = 1024;
    if is_dir {
        "<DIR>".to_string()
    } else if size < K {
        format!("{size} B")
    } else if size < K * K {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < K * K * K {
        format!("{:.1} MB", size as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", size as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

/// Do `a` and `b` name the same file, as far as spelling can tell?
///
/// CLAUDE.md rule 15: never `==` on two paths. This is the single entry
/// point the whole fs surface uses for "is this file already open in a
/// pane?" and "is this the directory the pane shows?", and it is on the Rust
/// side on purpose — a second, differently-opinionated case rule in
/// TypeScript is how the two layers come to disagree.
///
/// A `false` means "not provably the same", not "definitely different":
/// symlinks, `..` segments and 8.3 short names are out of scope, which is
/// why `fsops` still reaches for `canonicalize` where a wrong answer would
/// cost data.
pub fn same_file(a: &str, b: &str) -> bool {
    ypath::same_path(a, b)
}

/// Is `path` inside `dir`, or `dir` itself?
///
/// **Lexical, and not a sandbox.** It compares [`ypath::comparison_key`]s, so
/// it gets composition, case and separators right, and gets symlinks wrong
/// by construction: a link *inside* `dir` whose target is outside still
/// answers `true`, because its spelling is inside. Nothing on this surface
/// treats it as a boundary check — there is no path allow-list to escape
/// (spec §1.5 rule 5) — it exists to answer "would this copy land in its own
/// subtree?" and "does this pane still show this file?".
///
/// Compares whole components, so `C:\foo` does not contain `C:\foobar`.
pub fn is_within(dir: &str, path: &str) -> bool {
    let d = ypath::comparison_key(dir);
    let p = ypath::comparison_key(path);
    if d == p {
        return true;
    }
    // A root key is `/` or `c:/`; trimming leaves `` or `c:`, and the
    // separator test below then lands on the root's own slash.
    let prefix = d.trim_end_matches('/');
    p.len() > prefix.len() && p.starts_with(prefix) && p.as_bytes()[prefix.len()] == b'/'
}

/// [`is_within`]`(dir, p)` for each of `paths`, in order. The git pane asks
/// this before removing a worktree: a pane working inside it (on Windows a
/// shell's cwd cannot even be deleted) must move out first.
pub fn within_each(dir: &str, paths: &[String]) -> Vec<bool> {
    paths.iter().map(|p| is_within(dir, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> DirEntryInfo {
        DirEntryInfo {
            name: name.to_string(),
            path: format!("/x/{name}"),
            is_dir,
            is_symlink: false,
            size: 0,
            modified_ms: 0,
        }
    }

    fn names(entries: &[DirEntryInfo]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn sort_puts_directories_first_then_folds_case() {
        let mut v = vec![
            entry("zebra.txt", false),
            entry("Apple", true),
            entry("beta.txt", false),
            entry("aardvark", true),
            entry("Alpha.txt", false),
        ];
        sort_entries(&mut v);
        assert_eq!(
            names(&v),
            vec!["aardvark", "Apple", "Alpha.txt", "beta.txt", "zebra.txt"]
        );
    }

    /// Hangul sorts by code point after the fold, which is what ydir does;
    /// the point of the test is that it is stable and does not panic on
    /// multi-byte names.
    #[test]
    fn sort_handles_hangul_names() {
        let mut v = vec![
            entry("나중.txt", false),
            entry("가장", true),
            entry("문서.txt", false),
        ];
        sort_entries(&mut v);
        assert_eq!(names(&v), vec!["가장", "나중.txt", "문서.txt"]);
    }

    #[test]
    fn hidden_rule_is_the_leading_dot() {
        assert!(is_hidden_name(".git"));
        assert!(is_hidden_name(".env"));
        assert!(!is_hidden_name("a.txt"));
        // Any leading dot hides, including a doubled one.
        assert!(is_hidden_name("..a"));
        // `.` and `..` are never entries, but must not be called hidden.
        assert!(!is_hidden_name("."));
        assert!(!is_hidden_name(".."));
    }

    #[test]
    fn apply_hidden_only_filters_when_asked() {
        let mut v = vec![
            entry(".git", true),
            entry("src", true),
            entry("a.txt", false),
        ];
        let all = v.clone();

        apply_hidden(&mut v, true);
        assert_eq!(v, all);

        apply_hidden(&mut v, false);
        assert_eq!(names(&v), vec!["src", "a.txt"]);
    }

    /// The boundaries, because an off-by-one here is the difference between
    /// `1024 B` and `1.0 KB`.
    #[test]
    fn size_formatting_boundaries() {
        assert_eq!(format_size(0, false), "0 B");
        assert_eq!(format_size(1023, false), "1023 B");
        assert_eq!(format_size(1024, false), "1.0 KB");
        assert_eq!(format_size(1024 * 1024 - 1, false), "1024.0 KB");
        assert_eq!(format_size(1024 * 1024, false), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024, false), "1.0 GB");
        // A directory's own byte count is meaningless, so it is never shown.
        assert_eq!(format_size(4096, true), "<DIR>");
    }

    #[test]
    fn binary_sniff_finds_a_nul_and_nothing_else() {
        assert!(!is_probably_binary(b""));
        assert!(!is_probably_binary(b"plain text\n"));
        assert!(is_probably_binary(b"PK\x03\x04\x00stuff"));
        assert!(is_probably_binary(&[0u8]));
    }

    /// A UTF-8 Hangul sample is text, and so is one the read cap cut in the
    /// middle of a syllable — an incomplete trailing sequence is not a NUL
    /// and must not be mistaken for one.
    #[test]
    fn binary_sniff_calls_hangul_text_even_when_cut_mid_syllable() {
        let sample = "안녕하세요 여러분\n".repeat(64);
        assert!(!is_probably_binary(sample.as_bytes()));

        let bytes = sample.as_bytes();
        // The sample ends `분\n`; "분" is three bytes, so dropping two
        // leaves a truncated syllable as the tail — exactly what an 8 KiB
        // read cap does to a Hangul file.
        let cut = &bytes[..bytes.len() - 2];
        assert!(!is_probably_binary(cut));
        assert!(std::str::from_utf8(cut).is_err(), "the cut must be invalid");
    }

    /// Only the sniff window counts: a NUL past it is not looked at, which
    /// is what keeps the listing's answer and the preview's answer identical.
    #[test]
    fn binary_sniff_stops_at_the_window() {
        let mut v = vec![b'a'; BINARY_SNIFF_BYTES];
        v.push(0);
        assert!(!is_probably_binary(&v));

        let mut v = vec![b'a'; BINARY_SNIFF_BYTES - 1];
        v.push(0);
        assert!(is_probably_binary(&v));
    }

    /// The rule-15 respellings, routed through `ypath` rather than re-decided
    /// here.
    #[test]
    fn same_file_across_the_rule_15_respellings() {
        // Drive-letter case and separator: one file.
        assert!(same_file(r"C:\Repo\src", "c:/repo/src"));
        // A POSIX root is case-sensitive: two files.
        assert!(!same_file("/srv/A", "/srv/a"));
        // NFD vs NFC Hangul: one directory.
        assert!(same_file(
            "/srv/\u{d55c}\u{ae00}",
            "/srv/\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}"
        ));
        // A backslash is a legal POSIX filename character, so it is not a
        // separator until the path proves Windows syntax.
        assert!(!same_file(r"/srv/a\b", "/srv/a/b"));
    }

    #[test]
    fn is_within_compares_whole_components() {
        assert!(is_within(r"C:\foo", r"C:\foo\bar.txt"));
        assert!(is_within(r"C:\foo", r"c:/FOO/bar.txt"));
        // The boundary case a `starts_with` on raw strings gets wrong.
        assert!(!is_within(r"C:\foo", r"C:\foobar\x.txt"));
        assert!(!is_within("/srv/foo", "/srv/foobar/x"));
        // Outside, and the other way round.
        assert!(!is_within(r"C:\foo", r"C:\bar"));
        assert!(!is_within(r"C:\foo\bar", r"C:\foo"));
    }

    /// The git pane's "is any pane working in this worktree?": git's
    /// spelling of the worktree against a shell's OSC 7 cwd, a files pane's
    /// dir and an editor's file, across rule 15's respellings.
    #[test]
    fn within_each_answers_per_path_across_producers() {
        let nfd = "C:\\wt\\\u{1112}\u{1161}\u{11ab}\\src"; // 한 decomposed
        let got = within_each(
            "C:/wt/한",
            &[
                r"c:\WT\한".to_string(),
                nfd.to_string(),
                r"C:\wt\한\notes.md".to_string(),
                r"C:\wt\한글".to_string(),
                r"C:\repo".to_string(),
            ],
        );
        assert_eq!(got, vec![true, true, true, false, false]);
        assert!(within_each("/srv/wt", &[]).is_empty());
    }

    #[test]
    fn is_within_handles_roots_and_identity() {
        assert!(is_within("/", "/anything/at/all"));
        assert!(is_within(r"C:\", r"C:\Windows"));
        // A directory contains itself, however it is spelled.
        assert!(is_within(r"C:\foo", r"C:\foo\"));
        assert!(is_within("/srv", "/srv"));
    }

    /// The documented limitation, pinned so nobody later mistakes this for a
    /// sandbox: a symlink inside `dir` pointing out of it still reads as
    /// inside, because only the spelling is consulted.
    #[test]
    fn is_within_is_lexical_and_says_nothing_about_link_targets() {
        // `/srv/app/escape` may well be a link to `/etc`; its spelling is
        // inside `/srv/app`, and that is all this answers.
        assert!(is_within("/srv/app", "/srv/app/escape"));
        // A `..` segment is not resolved either.
        assert!(is_within("/srv/app", "/srv/app/../../etc/passwd"));
    }
}
