//! Streaming parser for OSC 7 `ESC ] 7 ; file://host/path ST` sequences.
//!
//! OSC 7 is the de-facto "current working directory" escape sequence used by
//! Windows Terminal, VS Code, WezTerm, iTerm2, and friends. When a shell
//! prompt emits it, the surrounding terminal multiplexer can track the
//! shell's live `cwd` without polling the process itself.
//!
//! This parser is deliberately minimal. It is a pure byte state machine so
//! it can be fed arbitrarily chunked PTY output (including splits inside an
//! escape sequence) without needing to buffer more than the in-flight OSC
//! body. Any unrecognised OSC number or malformed sequence is silently
//! dropped — the whole thing is a best-effort `cwd` hint, not an RPC
//! channel.

const MAX_BODY: usize = 4096;

pub struct Osc7Parser {
    state: State,
    buf: Vec<u8>,
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum State {
    /// Scanning for the start of a new escape sequence.
    Normal,
    /// Saw `ESC` (0x1b).
    Esc,
    /// Saw `ESC ]` — in an OSC introducer, waiting for the Ps number.
    OscIntroducer,
    /// Saw `ESC ] 7` — need the `;` separator next.
    Osc7Seen,
    /// Saw `ESC ] 7 ;` — now accumulating the body (URI) until ST.
    Osc7Body,
    /// Saw `ESC ] 7 ; ... ESC` — need a `\\` next to complete ST.
    Osc7BodyEsc,
}

impl Default for Osc7Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc7Parser {
    pub fn new() -> Self {
        Self {
            state: State::Normal,
            buf: Vec::with_capacity(256),
        }
    }

    /// Feed a chunk of bytes from the PTY master. Returns every complete
    /// OSC 7 `cwd` that was parsed in this chunk, decoded back into a
    /// platform-native filesystem path.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.state {
                State::Normal => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => match b {
                    b']' => self.state = State::OscIntroducer,
                    0x1b => { /* stay in Esc */ }
                    _ => self.state = State::Normal,
                },
                State::OscIntroducer => match b {
                    b'7' => self.state = State::Osc7Seen,
                    // Other OSC numbers (OSC 0 window title, etc.) — abandon,
                    // since we only care about OSC 7.
                    _ => self.state = State::Normal,
                },
                State::Osc7Seen => match b {
                    b';' => {
                        self.buf.clear();
                        self.state = State::Osc7Body;
                    }
                    _ => self.state = State::Normal,
                },
                State::Osc7Body => match b {
                    0x07 => {
                        // BEL terminator
                        if let Some(path) = decode(&self.buf) {
                            out.push(path);
                        }
                        self.buf.clear();
                        self.state = State::Normal;
                    }
                    0x1b => self.state = State::Osc7BodyEsc,
                    _ => {
                        if self.buf.len() < MAX_BODY {
                            self.buf.push(b);
                        } else {
                            // Over-long body — give up on this sequence.
                            self.buf.clear();
                            self.state = State::Normal;
                        }
                    }
                },
                State::Osc7BodyEsc => match b {
                    b'\\' => {
                        // ST terminator (ESC \)
                        if let Some(path) = decode(&self.buf) {
                            out.push(path);
                        }
                        self.buf.clear();
                        self.state = State::Normal;
                    }
                    _ => {
                        // Wasn't ST after all; treat the ESC as part of the
                        // body and reprocess the current byte as body.
                        if self.buf.len() < MAX_BODY {
                            self.buf.push(0x1b);
                            self.buf.push(b);
                        }
                        self.state = State::Osc7Body;
                    }
                },
            }
        }
        out
    }
}

/// Remembers the last cwd a pane reported, so the reader forwards only real
/// changes. Shells re-emit OSC 7 on every prompt, and without this the file
/// dock would re-navigate on every Enter.
///
/// Only the comparison *key* is remembered, never handed back out: the raw
/// cwd string is what travels on to the frontend.
#[derive(Debug, Default)]
pub struct CwdChange {
    last_key: Option<String>,
}

impl CwdChange {
    /// `true` when `cwd` names a different directory than the previous
    /// report. Records it.
    ///
    /// Compared through [`ypath::comparison_key`] rather than byte-wise. A
    /// shell can respell the same directory between two prompts -- the
    /// drive-letter case after `cd c:\x`, a trailing separator -- and on
    /// macOS the same Korean directory name arrives composed from one source
    /// and decomposed from another. Each respelling would otherwise look like
    /// a `cd` and re-navigate the file dock.
    pub fn is_new(&mut self, cwd: &str) -> bool {
        let key = ypath::comparison_key(cwd);
        if self.last_key.as_deref() == Some(key.as_str()) {
            return false;
        }
        self.last_key = Some(key);
        true
    }
}

/// Decode an OSC 7 payload into a platform-native path. The payload is
/// expected to be `file://<host>/<url-encoded-path>`. Any non-file URI or
/// decode failure returns `None`.
///
/// A network or device path is refused outright: a cwd is local. OSC 7 is
/// not trusted input — any output the pane prints (`cat` of a planted file,
/// a remote host over SSH) can emit it, and nothing resets it afterwards.
/// `file://x//evil/share/` decodes to `\\evil\share` on Windows, and with
/// that as the cwd every relative path the terminal linkifier probed on
/// hover would make Windows contact the attacker's server and offer NTLM
/// credentials. The rule is [`crate::fspath::cwd_is_local`]; every UNC,
/// `\\?\UNC\`, `\??\` and `\\.\` spelling is rejected, and the pane keeps
/// its previous cwd.
fn decode(buf: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(buf).ok()?;
    let rest = s.strip_prefix("file://")?;
    // The hostname runs up to the first `/`. Everything from the `/` onward
    // is the URL-encoded path.
    let slash = rest.find('/')?;
    let encoded_path = &rest[slash..];
    let decoded = url_decode(encoded_path)?;
    let path = normalize_windows(&decoded);
    crate::fspath::cwd_is_local(&path).then_some(path)
}

/// Minimal percent-decoder. Accepts `%HH` escapes and decodes them to raw
/// bytes; everything else passes through untouched. Returns `None` if the
/// resulting bytes are not valid UTF-8.
fn url_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Strip the leading `/` from `/C:/path` style Windows file URIs, convert
/// MSYS-style `/c/path` paths emitted by Git Bash into `C:\path`, and
/// normalise separators. On non-Windows hosts this is a no-op so unit
/// tests running on Linux observe forward-slash paths verbatim.
fn normalize_windows(path: &str) -> String {
    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        // `/C:/foo` (standard file:/// form) → `C:/foo`
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            return path[1..].replace('/', "\\");
        }
        // `/c/foo` (Git Bash / MSYS form) → `C:\foo`
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
        {
            let drive = (bytes[1] as char).to_ascii_uppercase();
            return format!("{drive}:{}", path[2..].replace('/', "\\"));
        }
        path.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        let _ = path; // suppress unused warning is not needed — `path` is used
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> Vec<String> {
        let mut p = Osc7Parser::new();
        p.feed(bytes)
    }

    #[test]
    fn parses_st_terminated_sequence() {
        let input = b"\x1b]7;file://host/home/alice\x1b\\";
        let out = parse_all(input);
        assert_eq!(out, vec!["/home/alice".to_string()]);
    }

    #[test]
    fn parses_bel_terminated_sequence() {
        let input = b"\x1b]7;file://host/tmp\x07";
        let out = parse_all(input);
        assert_eq!(out, vec!["/tmp".to_string()]);
    }

    #[test]
    fn ignores_other_osc_numbers() {
        // OSC 0 (window title) should be ignored.
        let input = b"\x1b]0;My Window\x07extra\x1b]7;file://h/foo\x07";
        let out = parse_all(input);
        assert_eq!(out, vec!["/foo".to_string()]);
    }

    #[test]
    fn handles_chunked_input() {
        let input = b"\x1b]7;file://host/home/alice\x1b\\";
        let mut p = Osc7Parser::new();
        let mut collected = Vec::new();
        for chunk in input.chunks(3) {
            collected.extend(p.feed(chunk));
        }
        assert_eq!(collected, vec!["/home/alice".to_string()]);
    }

    #[test]
    fn decodes_percent_escapes() {
        let input = b"\x1b]7;file://host/path%20with%20spaces\x07";
        let out = parse_all(input);
        assert_eq!(out, vec!["/path with spaces".to_string()]);
    }

    #[test]
    fn extracts_latest_cwd_when_many_prompts() {
        let input = b"hi\x1b]7;file://h/a\x07more\x1b]7;file://h/b\x07tail";
        let out = parse_all(input);
        assert_eq!(out, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn malformed_sequence_is_dropped() {
        let input = b"\x1b]7;file://host/path-no-terminator...then garbage";
        let out = parse_all(input);
        assert!(out.is_empty());
    }

    #[test]
    fn non_file_scheme_is_ignored() {
        let input = b"\x1b]7;http://example.com/foo\x07";
        let out = parse_all(input);
        assert!(out.is_empty());
    }

    #[test]
    fn accepts_empty_hostname() {
        let input = b"\x1b]7;file:///usr/local\x07";
        let out = parse_all(input);
        assert_eq!(out, vec!["/usr/local".to_string()]);
    }

    #[test]
    fn normal_text_does_not_trigger_sequences() {
        let input = b"hello world\n";
        let out = parse_all(input);
        assert!(out.is_empty());
    }

    #[test]
    fn long_body_is_bounded() {
        let mut input = b"\x1b]7;file://host/".to_vec();
        input.extend(std::iter::repeat(b'a').take(MAX_BODY + 100));
        input.push(0x07);
        // Should not crash, parser should reset and recover. We don't care
        // whether a path was produced; the important thing is that feed
        // returns and the parser is in a sensible state afterwards.
        let mut p = Osc7Parser::new();
        let _ = p.feed(&input);
        let out = p.feed(b"\x1b]7;file://h/ok\x07");
        assert_eq!(out, vec!["/ok".to_string()]);
    }

    /// A planted OSC 7 must not be able to make a share the pane's cwd: the
    /// terminal linkifier resolves relative paths against it on hover, and on
    /// Windows that stat offers NTLM credentials to whoever owns the host.
    #[test]
    fn a_network_or_device_cwd_is_refused() {
        // Refused on every platform: the backslash forms are never a cwd a
        // real shell reports, and on Windows they are shares or devices.
        for payload in [
            "file://x/%5C%5Cevil%5Cshare",
            "file://x/%5C%5C%3F%5CUNC%5Cevil%5Cshare",
        ] {
            let seq = format!("\x1b]7;{payload}\x07");
            // `/\\evil\share` is a legal POSIX name, so only Windows refuses
            // this one; everywhere the result must not *be* a share.
            for cwd in parse_all(seq.as_bytes()) {
                assert!(crate::fspath::cwd_is_local(&cwd), "{payload} -> {cwd}");
            }
        }
        if cfg!(windows) {
            for payload in [
                // The review's example: host `x`, path `//evil/share/`.
                "file://x//evil/share/",
                "file:////evil/share",
                "file:///%5C%5Cevil%5Cshare",
                "file:///??/UNC/evil/share",
                "file:///%3F%3F/UNC/evil/share",
                "file:////?/UNC/evil/share",
                "file:////./UNC/evil/share",
                "file:////./PhysicalDrive0",
                "file:////?/GLOBALROOT/Device/Mup/evil/share",
                // A pseudo-local host is still not a local directory.
                "file:////wsl$/Ubuntu/home",
                "file:////localhost/c$/Windows",
            ] {
                let seq = format!("\x1b]7;{payload}\x07");
                assert!(parse_all(seq.as_bytes()).is_empty(), "{payload}");
            }
            // A local drive still decodes.
            assert_eq!(
                parse_all(b"\x1b]7;file:///C:/Users/me\x07"),
                vec![r"C:\Users\me".to_string()]
            );
        }
    }

    #[test]
    fn cwd_change_reports_only_real_changes() {
        // Shells re-emit OSC 7 on every prompt, so the same dir must not
        // count as a change, while going back to an earlier dir must.
        let mut c = CwdChange::default();
        assert!(c.is_new("/a"));
        assert!(!c.is_new("/a"));
        assert!(c.is_new("/b"));
        assert!(c.is_new("/a"));
    }

    /// The same directory respelled is not a `cd`. macOS reports a decomposed
    /// Hangul name where the shell's own OSC 7 payload carries the composed
    /// one, and a Windows shell is free to change the drive-letter case or
    /// tack on a trailing separator between prompts.
    #[test]
    fn cwd_change_ignores_respellings_of_the_same_directory() {
        const NFC: &str = "한글";
        const NFD: &str = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}";
        assert_ne!(NFC, NFD, "fixture is wrong: the spellings must differ");

        let mut c = CwdChange::default();
        assert!(c.is_new(&format!("/Users/u/{NFC}")));
        assert!(!c.is_new(&format!("/Users/u/{NFD}")));

        let mut w = CwdChange::default();
        assert!(w.is_new(r"C:\Work\Repo"));
        assert!(!w.is_new("c:/work/repo/"));

        // ...but a genuinely different POSIX directory still reports, even
        // when it differs only in case: `/srv/A` and `/srv/a` are two dirs.
        let mut p = CwdChange::default();
        assert!(p.is_new("/srv/a"));
        assert!(p.is_new("/srv/A"));
    }
}
