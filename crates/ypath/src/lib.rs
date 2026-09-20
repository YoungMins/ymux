//! Comparison keys for paths that arrive from different sources.
//!
//! **The key is for comparison only. Never open it, store it, hand it to
//! `git`, or splice it back into a real path.** It folds case, separators and
//! Unicode composition, so it is lossy by construction — the raw string the
//! caller received is the only thing that still names the file.
//!
//! Two problems it solves:
//!
//! 1. **Unicode composition.** macOS hands back decomposed (NFD) Korean and
//!    Japanese filenames, while a shell's OSC 7 report, a config file or a
//!    `git` listing usually carries the composed (NFC) spelling. `"한글"` from
//!    two sources is then byte-different and compares unequal even though both
//!    name the same directory. The key normalizes to NFC first.
//!
//! 2. **Case sensitivity, decided by path *syntax* — never by `cfg!(windows)`.**
//!    A Windows ymux drives WSL and SSH shells, so the host platform says
//!    nothing about the path in hand. A drive path (`C:\…`) or a UNC share
//!    folds case; a POSIX path does not — *including on macOS*: folding a
//!    case-sensitive root merges distinct files, which is worse than missing a
//!    case-only duplicate. The WSL UNC aliases (`\\wsl$\…`,
//!    `\\wsl.localhost\…`) front a case-sensitive Linux filesystem, so only
//!    their distro segment folds.
//!
//! Separators follow the same rule: `\` is a legal POSIX filename character,
//! so it is folded to `/` only once the path has proved Windows semantics.

use unicode_normalization::UnicodeNormalization;

/// Whether `path` proves Windows drive or UNC semantics by its own spelling:
/// `C:\…` / `C:/…`, or a `\\…` / `//…` UNC-style root.
///
/// Deliberately syntax-only — a `C:foo` drive-*relative* path is not included,
/// matching the "prove it" rule the module doc describes.
pub fn is_windows_path_like(path: &str) -> bool {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    {
        return true;
    }
    path.starts_with("\\\\") || path.starts_with("//")
}

/// Comparison key for `path`. See the module doc: **never a real path.**
///
/// NFC first, then: a path that proves Windows syntax gets `\` folded to `/`,
/// redundant separators dropped and case folded over the segments Windows
/// itself owns; anything else is treated as POSIX and only loses redundant
/// `/` separators.
pub fn comparison_key(path: &str) -> String {
    let nfc: String = path.nfc().collect();
    if is_windows_path_like(&nfc) {
        windows_key(&nfc)
    } else {
        posix_key(&nfc)
    }
}

/// POSIX: `/` is the only separator and every byte after it is significant.
/// Collapses `//` runs and drops a trailing `/`, keeping a lone root `/`.
fn posix_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    if s.starts_with('/') {
        out.push('/');
    }
    let mut first = true;
    for seg in s.split('/').filter(|seg| !seg.is_empty()) {
        if !first {
            out.push('/');
        }
        out.push_str(seg);
        first = false;
    }
    out
}

/// Windows: `\` and `/` are interchangeable, and the segments the Win32
/// namespace owns compare case-insensitively.
///
/// `to_lowercase` rather than a true case-folding table: Windows folds with
/// its own uppercase table, so exotic scripts can in principle disagree at
/// the margins. For the drive letters, share names and ASCII directory names
/// this key is used on, the two agree.
fn windows_key(s: &str) -> String {
    let s = s.replace('\\', "/");

    // UNC / device paths: `//server/share/…`, `//?/C:/…`, `//wsl$/distro/…`.
    if let Some(rest) = s.strip_prefix("//") {
        let mut segs = rest.split('/').filter(|seg| !seg.is_empty());
        let server = segs.next().unwrap_or("").to_lowercase();

        // The WSL aliases are two spellings of one gateway onto a
        // *case-sensitive* Linux filesystem. Canonicalize the alias, fold the
        // distro (a Windows-side share name) and then stop folding.
        if server == "wsl$" || server == "wsl.localhost" {
            let distro = segs.next().unwrap_or("").to_lowercase();
            let mut out = format!("//wsl$/{distro}");
            for seg in segs {
                out.push('/');
                out.push_str(seg);
            }
            return out;
        }

        let mut out = format!("//{server}");
        for seg in segs {
            out.push('/');
            out.push_str(&seg.to_lowercase());
        }
        return out;
    }

    // Drive path: `C:/…`. The root keeps its separator (`c:/`), because `c:`
    // alone would be the drive-*relative* path, a different thing entirely.
    let mut segs = s.split('/').filter(|seg| !seg.is_empty());
    let drive = segs.next().unwrap_or("").to_lowercase();
    let mut out = format!("{drive}/");
    let mut first = true;
    for seg in segs {
        if !first {
            out.push('/');
        }
        out.push_str(&seg.to_lowercase());
        first = false;
    }
    out
}

/// Whether `a` and `b` name the same path as far as [`comparison_key`] can
/// tell. A `false` means "not provably the same", not "definitely different" —
/// symlinks, `..` segments and 8.3 short names are out of scope.
pub fn same_path(a: &str, b: &str) -> bool {
    comparison_key(a) == comparison_key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Composed vs. decomposed Hangul: the same two syllables, different bytes.
    const NFC_HANGUL: &str = "한글";
    const NFD_HANGUL: &str = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}";

    #[test]
    fn decomposed_and_composed_hangul_compare_equal() {
        assert_ne!(
            NFC_HANGUL, NFD_HANGUL,
            "fixture is wrong: the two spellings must differ byte-wise"
        );
        assert!(same_path(
            &format!("/home/u/{NFC_HANGUL}"),
            &format!("/home/u/{NFD_HANGUL}")
        ));
        assert!(same_path(
            &format!("C:\\Users\\u\\{NFC_HANGUL}"),
            &format!("C:/Users/u/{NFD_HANGUL}")
        ));
    }

    #[test]
    fn windows_drive_paths_fold_case_and_separators() {
        assert!(same_path("C:\\A\\b", "c:\\a\\B"));
        assert!(same_path("C:\\A\\b", "c:/a/b"));
    }

    #[test]
    fn posix_paths_stay_case_sensitive() {
        assert!(!same_path("/home/a", "/home/A"));
        assert!(same_path("/home/a", "/home/a"));
    }

    #[test]
    fn a_backslash_in_a_posix_path_is_a_filename_character() {
        // `a\b` is one file named `a\b`, not `a` containing `b`.
        assert_eq!(comparison_key("/home/a\\b"), "/home/a\\b");
        assert!(!same_path("/home/a\\b", "/home/a/b"));
        // Relative paths prove nothing about their flavor either.
        assert_eq!(comparison_key("a\\b"), "a\\b");
    }

    #[test]
    fn unc_shares_fold_case_and_separators() {
        assert!(same_path("\\\\Server\\Share\\Dir", "//server/share/dir"));
        assert_eq!(
            comparison_key("\\\\Server\\Share\\Dir"),
            "//server/share/dir"
        );
    }

    #[test]
    fn verbatim_prefix_paths_fold_like_the_drive_paths_they_wrap() {
        assert_eq!(comparison_key("\\\\?\\C:\\Users\\A"), "//?/c:/users/a");
        assert!(same_path("\\\\?\\C:\\Users\\A", "\\\\?\\c:/users/a"));
    }

    #[test]
    fn wsl_unc_aliases_agree_but_the_linux_tail_stays_case_sensitive() {
        // Both spellings front the same distro.
        assert!(same_path(
            "\\\\wsl$\\Ubuntu\\home\\a",
            "\\\\wsl.localhost\\ubuntu\\home\\a"
        ));
        // The distro name is a Windows-side share name, so it folds...
        assert!(same_path(
            "\\\\wsl$\\Ubuntu\\home\\a",
            "\\\\wsl$\\UBUNTU\\home\\a"
        ));
        // ...while the Linux path it fronts does not.
        assert!(!same_path(
            "\\\\wsl$\\Ubuntu\\home\\a",
            "\\\\wsl$\\Ubuntu\\home\\A"
        ));
        // A different distro is a different filesystem.
        assert!(!same_path(
            "\\\\wsl$\\Ubuntu\\home\\a",
            "\\\\wsl$\\Debian\\home\\a"
        ));
    }

    #[test]
    fn trailing_and_duplicate_separators_are_ignored_but_roots_survive() {
        assert!(same_path("C:\\a\\", "C:/a"));
        assert!(same_path("/home//u/", "/home/u"));
        assert_eq!(comparison_key("/"), "/");
        assert_eq!(comparison_key("C:\\"), "c:/");
        assert_eq!(comparison_key(""), "");
    }

    #[test]
    fn is_windows_path_like_needs_a_separator_after_the_drive_colon() {
        assert!(is_windows_path_like("C:\\x"));
        assert!(is_windows_path_like("c:/x"));
        assert!(is_windows_path_like("\\\\server\\share"));
        assert!(!is_windows_path_like("C:relative"));
        assert!(!is_windows_path_like("/home/u"));
        assert!(!is_windows_path_like("a\\b"));
    }
}
