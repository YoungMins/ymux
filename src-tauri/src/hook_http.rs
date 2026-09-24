//! Loopback HTTP receiver for Claude Code's `type: "http"` hooks.
//!
//! Claude Code POSTs each hook event's JSON to
//! `http://127.0.0.1:<port>/ymux-agent-hook` (the entries `agent_hooks`
//! installs), with two headers it interpolates from the Claude process's own
//! environment: `X-Ymux-Pane: ${YMUX_PANE_ID}` and
//! `X-Ymux-Token: ${YMUX_HOOK_TOKEN}`. ymux injects both into every PTY, so
//! only a Claude running inside one of this ymux's panes can produce a
//! request that is accepted. See CLAUDE.md rule 13 for the contract.
//!
//! Everything here is pure `std` and not desktop-gated, so the request
//! parsing, the auth decision, the port choice and a real listener round-trip
//! are all tested under `cargo test --no-default-features --lib -p ymux`.
//! Only the glue that feeds accepted events into the Tauri-managed
//! `AgentRegistry` lives in `main.rs`.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::agents::HookEvent;

/// The one path the receiver answers on. Also the ownership marker of every
/// hook entry `agent_hooks` installs (CLAUDE.md rule 12).
pub const HOOK_PATH: &str = "/ymux-agent-hook";
/// Header carrying `$YMUX_PANE_ID` (lower-case: header names are matched
/// case-insensitively).
pub const PANE_HEADER: &str = "x-ymux-pane";
/// Header carrying `$YMUX_HOOK_TOKEN`.
pub const TOKEN_HEADER: &str = "x-ymux-token";
/// Env var that carries this run's token into every PTY.
pub const TOKEN_ENV: &str = "YMUX_HOOK_TOKEN";

/// Cap on the request line + headers.
pub const MAX_HEAD: usize = 16 * 1024;
/// Cap on the JSON body. Generous: a `PreToolUse` for a `Write` carries the
/// whole file in `tool_input`. A bigger (authenticated) body is dropped with
/// an empty 204 rather than refused, because any non-2xx would show up as a
/// hook error in the user's Claude session.
pub const MAX_BODY: usize = 8 * 1024 * 1024;
/// Per-connection read/write timeout. Claude sends the whole request at once
/// over loopback; anything slower is not Claude.
pub const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Optional hook fields forwarded when present (what `y agent-hook` packed).
const OPTIONAL_FIELDS: &[&str] = &["agent_id", "agent_type", "tool_name"];

/// A fresh per-run secret: two v4 UUIDs from the OS CSPRNG, 244 random bits,
/// as 64 hex characters (header- and env-safe).
pub fn new_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Constant-time equality for the token check: the time taken depends only
/// on the lengths, never on where the first differing byte is.
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Request line + headers of one HTTP/1.x request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    /// `(lower-cased name, trimmed value)` in arrival order.
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    /// First value of header `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Parse the bytes before the blank line (`\r\n\r\n` excluded). `None` for
/// anything that isn't a well-formed HTTP/1.x request head.
pub fn parse_head(bytes: &[u8]) -> Option<RequestHead> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, path, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || method.is_empty() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.is_empty() || name.contains(char::is_whitespace) {
            return None;
        }
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    Some(RequestHead {
        method: method.to_string(),
        path: path.to_string(),
        headers,
    })
}

/// What to do with a request, decided from its head alone — the body of a
/// request that isn't accepted is never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Read `len` body bytes and apply them to `pane`.
    Accept { pane: Uuid, len: usize },
    /// Answer an empty 204 and do nothing.
    Ignore,
    /// Answer this non-2xx status with an empty body.
    Reject(u16),
}

/// The auth decision (CLAUDE.md rule 13). In order:
/// 1. an `Origin` header → 403. Browsers attach it to every cross-origin
///    POST; Claude Code's client never sends one;
/// 2. a `Host` that isn't loopback → 403 (DNS-rebinding belt and braces);
/// 3. not `POST` → 405, not [`HOOK_PATH`] → 404;
/// 4. pane *and* token header both empty → 204, ignored: that is a Claude
///    session started outside ymux (its env has neither variable) while
///    ymux happens to run, and a non-2xx would put a hook error in it;
/// 5. wrong token (constant-time compare), or a pane that isn't a UUID or
///    isn't one of ours → 403;
/// 6. no `Content-Length` or any `Transfer-Encoding` → 411;
/// 7. body over [`MAX_BODY`] → 204, dropped (see there).
pub fn authorize(head: &RequestHead, token: &str, known_pane: impl Fn(Uuid) -> bool) -> Verdict {
    if head.header("origin").is_some() {
        return Verdict::Reject(403);
    }
    if let Some(host) = head.header("host") {
        if !host_is_loopback(host) {
            return Verdict::Reject(403);
        }
    }
    if head.method != "POST" {
        return Verdict::Reject(405);
    }
    if head.path != HOOK_PATH {
        return Verdict::Reject(404);
    }
    let pane = head.header(PANE_HEADER).unwrap_or("");
    let sent = head.header(TOKEN_HEADER).unwrap_or("");
    if pane.is_empty() && sent.is_empty() {
        return Verdict::Ignore;
    }
    if !tokens_equal(sent, token) {
        return Verdict::Reject(403);
    }
    let Some(pane) = Uuid::parse_str(pane).ok().filter(|p| known_pane(*p)) else {
        return Verdict::Reject(403);
    };
    if head.header("transfer-encoding").is_some() {
        return Verdict::Reject(411);
    }
    let Some(len) = head
        .header("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return Verdict::Reject(411);
    };
    if len > MAX_BODY {
        return Verdict::Ignore;
    }
    Verdict::Accept { pane, len }
}

/// `127.0.0.1[:port]` or `localhost[:port]`.
fn host_is_loopback(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")
}

/// Build the agent-registry event for one hook body — the same shape
/// `y agent-hook` used to relay — or `None` if it isn't a hook payload.
pub fn hook_event(body: &[u8], pane: Uuid) -> Option<HookEvent> {
    let input: Value = serde_json::from_slice(body).ok()?;
    let event = input.get("hook_event_name")?.as_str()?;
    let session_id = input
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut payload = Map::new();
    payload.insert("pane_id".into(), pane.to_string().into());
    payload.insert("agent".into(), "claude".into());
    payload.insert("event".into(), event.into());
    payload.insert("session_id".into(), session_id.into());
    for key in OPTIONAL_FIELDS {
        if let Some(v) = input.get(*key).and_then(Value::as_str) {
            payload.insert((*key).into(), v.into());
        }
    }
    HookEvent::from_payload(&Value::Object(payload))
}

/// An empty-bodied response. Never a JSON body: on a 2xx Claude Code would
/// parse it as hook output (decisions, context), and a non-2xx is an error
/// whatever it carries.
pub fn response(status: u16) -> String {
    let reason = match status {
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

/// Bind the receiver: the persisted port when it is set and free, else a
/// fresh OS-assigned one. Returns the listener and whether it is on
/// `persisted` (`false` = the caller should persist the new port and
/// refresh the installed hooks). Generic over `bind` so the decision is
/// tested without depending on which ports happen to be free.
pub fn choose_port<L>(
    persisted: u16,
    mut bind: impl FnMut(u16) -> io::Result<L>,
) -> io::Result<(L, bool)> {
    if persisted != 0 {
        if let Ok(l) = bind(persisted) {
            return Ok((l, true));
        }
    }
    bind(0).map(|l| (l, false))
}

/// `TcpListener::bind` on `127.0.0.1:port` — loopback only, never
/// `0.0.0.0`.
pub fn bind_loopback(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

pub type KnownPane = Arc<dyn Fn(Uuid) -> bool + Send + Sync>;
pub type OnEvent = Arc<dyn Fn(HookEvent) + Send + Sync>;

/// Run the accept loop on a background thread for the life of the process.
/// Each connection is served on its own short-lived thread; accepted events
/// go through one channel to a single applier thread that calls `on_event`.
///
/// Order matters: Claude Code sends hook N+1 only after hook N's response,
/// so queueing each event *before* its response keeps the registry's order
/// equal to Claude's (a `Stop` can't overtake the last `PostToolUse`), while
/// the response still never waits for `on_event` and its locks.
pub fn serve(
    listener: TcpListener,
    token: String,
    known_pane: KnownPane,
    on_event: OnEvent,
) -> io::Result<std::thread::JoinHandle<()>> {
    let token: Arc<str> = token.into();
    let (tx, rx) = std::sync::mpsc::channel::<HookEvent>();
    std::thread::Builder::new()
        .name("ymux-hook-apply".into())
        .spawn(move || {
            for ev in rx {
                on_event(ev);
            }
        })?;
    std::thread::Builder::new()
        .name("ymux-hook-http".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (token, known_pane, tx) = (token.clone(), known_pane.clone(), tx.clone());
                let _ = std::thread::Builder::new()
                    .name("ymux-hook-conn".into())
                    .spawn(move || {
                        let _ = handle(stream, &token, &*known_pane, &tx);
                    });
            }
        })
}

/// Serve one request: queue the accepted event, then answer.
fn handle(
    mut stream: TcpStream,
    token: &str,
    known_pane: &(dyn Fn(Uuid) -> bool + Send + Sync),
    queue: &std::sync::mpsc::Sender<HookEvent>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return reply(&mut stream, 431);
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let Some(head) = parse_head(&buf[..head_end]) else {
        return reply(&mut stream, 400);
    };
    let (pane, len) = match authorize(&head, token, known_pane) {
        Verdict::Accept { pane, len } => (pane, len),
        Verdict::Ignore => return reply(&mut stream, 204),
        Verdict::Reject(status) => return reply(&mut stream, status),
    };
    let mut body = buf.split_off(head_end + 4);
    if body.len() < len {
        let have = body.len();
        body.resize(len, 0);
        stream.read_exact(&mut body[have..])?;
    }
    body.truncate(len);
    let Some(event) = hook_event(&body, pane) else {
        return reply(&mut stream, 400);
    };
    let _ = queue.send(event);
    reply(&mut stream, 204)
}

/// Upper bound on request bytes discarded after an early reply.
const MAX_DRAIN: u64 = MAX_BODY as u64 + MAX_HEAD as u64;

/// Write `status`, half-close, then read and discard whatever the client is
/// still sending (bounded by [`MAX_DRAIN`] and [`IO_TIMEOUT`]). Closing a
/// socket with unread bytes queued makes the OS send a reset instead of a
/// clean close, and a client still uploading a large body — a Claude outside
/// ymux posting a big `PostToolUse` — would see that as a failed hook.
fn reply(stream: &mut TcpStream, status: u16) -> io::Result<()> {
    stream.write_all(response(status).as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = io::copy(&mut (&*stream).take(MAX_DRAIN), &mut io::sink());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn pane() -> Uuid {
        Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()
    }

    /// The head Claude Code 2.1 actually sends (captured from a live
    /// `claude -p` run against a raw socket), minus `Accept*`.
    fn claude_head(extra: &[(&str, &str)]) -> RequestHead {
        let mut headers: Vec<(String, String)> = vec![
            ("content-type".into(), "application/json".into()),
            (PANE_HEADER.into(), pane().to_string()),
            (TOKEN_HEADER.into(), TOKEN.into()),
            ("user-agent".into(), "axios/1.15.2".into()),
            ("content-length".into(), "42".into()),
            ("host".into(), "127.0.0.1:41234".into()),
            ("connection".into(), "keep-alive".into()),
        ];
        for (k, v) in extra {
            headers.retain(|(n, _)| n != k);
            if !v.is_empty() || *k == PANE_HEADER || *k == TOKEN_HEADER {
                headers.push(((*k).into(), (*v).into()));
            }
        }
        RequestHead {
            method: "POST".into(),
            path: HOOK_PATH.into(),
            headers,
        }
    }

    fn ours(p: Uuid) -> bool {
        p == pane()
    }

    #[test]
    fn token_is_long_hex_and_fresh_each_time() {
        let a = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, new_token());
    }

    #[test]
    fn tokens_equal_is_exact() {
        assert!(tokens_equal(TOKEN, TOKEN));
        assert!(!tokens_equal(TOKEN, &TOKEN[1..]));
        assert!(!tokens_equal(TOKEN, &TOKEN.replace('f', "e")));
        assert!(!tokens_equal("", TOKEN));
    }

    #[test]
    fn parses_a_request_head_case_insensitively() {
        let head = parse_head(
            b"POST /ymux-agent-hook HTTP/1.1\r\nX-Ymux-Pane: abc\r\nContent-Length:  7 ",
        )
        .unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.path, HOOK_PATH);
        assert_eq!(head.header("x-ymux-pane"), Some("abc"));
        assert_eq!(head.header("CONTENT-LENGTH"), Some("7"));
    }

    #[test]
    fn rejects_malformed_heads() {
        assert!(parse_head(b"").is_none());
        assert!(parse_head(b"POST /x").is_none());
        assert!(parse_head(b"POST /x SPDY/3").is_none());
        assert!(parse_head(b"POST /x HTTP/1.1\r\nno-colon-here").is_none());
        assert!(parse_head(b"POST /x HTTP/1.1\r\nbad name: v").is_none());
        assert!(parse_head(&[0xff, 0xfe]).is_none());
    }

    #[test]
    fn accepts_what_claude_sends() {
        assert_eq!(
            authorize(&claude_head(&[]), TOKEN, ours),
            Verdict::Accept {
                pane: pane(),
                len: 42
            }
        );
        // `localhost` in Host is fine too.
        assert!(matches!(
            authorize(&claude_head(&[("host", "localhost:41234")]), TOKEN, ours),
            Verdict::Accept { .. }
        ));
    }

    #[test]
    fn any_origin_header_is_refused() {
        for origin in ["https://evil.example", "null", "http://127.0.0.1:41234"] {
            assert_eq!(
                authorize(&claude_head(&[("origin", origin)]), TOKEN, ours),
                Verdict::Reject(403),
                "{origin}"
            );
        }
    }

    #[test]
    fn a_non_loopback_host_is_refused() {
        assert_eq!(
            authorize(&claude_head(&[("host", "evil.example:41234")]), TOKEN, ours),
            Verdict::Reject(403)
        );
    }

    #[test]
    fn wrong_method_or_path_is_refused() {
        let mut get = claude_head(&[]);
        get.method = "GET".into();
        assert_eq!(authorize(&get, TOKEN, ours), Verdict::Reject(405));
        let mut other = claude_head(&[]);
        other.path = "/ymux-agent-hook/../x".into();
        assert_eq!(authorize(&other, TOKEN, ours), Verdict::Reject(404));
    }

    #[test]
    fn a_session_outside_ymux_is_ignored_quietly() {
        // No env → Claude interpolates both headers to "".
        let outside = claude_head(&[(PANE_HEADER, ""), (TOKEN_HEADER, "")]);
        assert_eq!(authorize(&outside, TOKEN, ours), Verdict::Ignore);
        let mut absent = claude_head(&[]);
        absent
            .headers
            .retain(|(n, _)| n != PANE_HEADER && n != TOKEN_HEADER);
        assert_eq!(authorize(&absent, TOKEN, ours), Verdict::Ignore);
    }

    #[test]
    fn a_wrong_or_missing_token_is_refused() {
        for bad in ["", "nope", &TOKEN[..31]] {
            assert_eq!(
                authorize(&claude_head(&[(TOKEN_HEADER, bad)]), TOKEN, ours),
                Verdict::Reject(403),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_unknown_or_malformed_pane_is_refused() {
        let other = Uuid::new_v4().to_string();
        for bad in ["", "not-a-uuid", other.as_str()] {
            assert_eq!(
                authorize(&claude_head(&[(PANE_HEADER, bad)]), TOKEN, ours),
                Verdict::Reject(403),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn body_framing_is_required_and_capped() {
        let mut no_len = claude_head(&[]);
        no_len.headers.retain(|(n, _)| n != "content-length");
        assert_eq!(authorize(&no_len, TOKEN, ours), Verdict::Reject(411));
        assert_eq!(
            authorize(
                &claude_head(&[("transfer-encoding", "chunked")]),
                TOKEN,
                ours
            ),
            Verdict::Reject(411)
        );
        let too_big = (MAX_BODY + 1).to_string();
        assert_eq!(
            authorize(&claude_head(&[("content-length", &too_big)]), TOKEN, ours),
            Verdict::Ignore
        );
    }

    #[test]
    fn hook_event_packs_the_fields_the_registry_needs() {
        let body = br#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
                        "agent_id":"a1","agent_type":"Explore","tool_input":{"command":"ls"}}"#;
        let ev = hook_event(body, pane()).unwrap();
        assert_eq!(ev.pane_id, pane());
        assert_eq!(ev.agent, "claude");
        assert_eq!(ev.event, "PreToolUse");
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.agent_id.as_deref(), Some("a1"));
        assert_eq!(ev.agent_type.as_deref(), Some("Explore"));
        assert_eq!(ev.tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn hook_event_omits_absent_fields_and_rejects_non_hooks() {
        let ev = hook_event(br#"{"hook_event_name":"Stop"}"#, pane()).unwrap();
        assert_eq!(ev.session_id, "");
        assert_eq!(ev.agent_id, None);
        assert!(hook_event(b"not json", pane()).is_none());
        assert!(hook_event(br#"{"session_id":"s"}"#, pane()).is_none());
    }

    #[test]
    fn responses_have_an_empty_body() {
        for status in [204, 400, 403, 404, 405, 411, 431] {
            let r = response(status);
            assert!(r.starts_with(&format!("HTTP/1.1 {status} ")), "{r}");
            assert!(r.ends_with("Content-Length: 0\r\nConnection: close\r\n\r\n"));
        }
    }

    #[test]
    fn choose_port_reuses_the_persisted_port_when_free() {
        let mut tried = Vec::new();
        let (l, reused) = choose_port(41234, |p| {
            tried.push(p);
            Ok(p)
        })
        .unwrap();
        assert_eq!((l, reused), (41234, true));
        assert_eq!(tried, vec![41234]);
    }

    #[test]
    fn choose_port_falls_back_to_a_fresh_port() {
        let mut tried = Vec::new();
        let (l, reused) = choose_port(41234, |p| {
            tried.push(p);
            if p == 0 {
                Ok(50000)
            } else {
                Err(io::ErrorKind::AddrInUse.into())
            }
        })
        .unwrap();
        assert_eq!((l, reused), (50000, false));
        assert_eq!(tried, vec![41234, 0]);
        // First run: nothing persisted, straight to a fresh port.
        let (_, reused) = choose_port(0, |_| Ok(1u16)).unwrap();
        assert!(!reused);
    }

    #[test]
    fn choose_port_with_real_sockets() {
        let taken = bind_loopback(0).unwrap();
        let busy = taken.local_addr().unwrap().port();
        let (l, reused) = choose_port(busy, bind_loopback).unwrap();
        assert!(!reused);
        let fresh = l.local_addr().unwrap();
        assert_ne!(fresh.port(), busy);
        assert!(fresh.ip().is_loopback());
        drop(l);
        drop(taken);
        let (again, reused) = choose_port(busy, bind_loopback).unwrap();
        assert!(reused);
        assert_eq!(again.local_addr().unwrap().port(), busy);
    }

    /// Start a real receiver on an ephemeral port; events arrive on the channel.
    fn start() -> (u16, mpsc::Receiver<HookEvent>) {
        let listener = bind_loopback(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let tx = parking_lot::Mutex::new(tx);
        serve(
            listener,
            TOKEN.into(),
            Arc::new(ours),
            Arc::new(move |ev| {
                let _ = tx.lock().send(ev);
            }),
        )
        .unwrap();
        (port, rx)
    }

    fn send(port: u16, raw: &[u8]) -> String {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn request(port: u16, pane: &str, token: &str, extra: &str, body: &str) -> Vec<u8> {
        format!(
            "POST {HOOK_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
             X-Ymux-Pane: {pane}\r\nX-Ymux-Token: {token}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn listener_round_trip() {
        let (port, rx) = start();
        let body = r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;
        let resp = send(port, &request(port, &pane().to_string(), TOKEN, "", body));
        assert_eq!(resp, response(204), "empty 2xx, never a JSON body");
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(ev.event, "UserPromptSubmit");
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.pane_id, pane());

        // Forged / foreign requests get their status and never reach the registry.
        let p = pane().to_string();
        let bad_token = send(port, &request(port, &p, "guess", "", body));
        assert!(bad_token.starts_with("HTTP/1.1 403 "), "{bad_token}");
        let from_page = send(
            port,
            &request(port, &p, TOKEN, "Origin: https://evil.example\r\n", body),
        );
        assert!(from_page.starts_with("HTTP/1.1 403 "), "{from_page}");
        let outside = send(port, &request(port, "", "", "", body));
        assert_eq!(outside, response(204));
        let garbage = send(port, b"hello\r\n\r\n");
        assert!(garbage.starts_with("HTTP/1.1 400 "), "{garbage}");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    /// A Claude outside ymux posting a big `PostToolUse` (a `Read` result):
    /// the quiet 204 must arrive intact, not as a connection reset from
    /// closing on an unread body.
    #[test]
    fn listener_ignores_a_large_body_without_resetting() {
        let (port, rx) = start();
        let big = format!(
            r#"{{"hook_event_name":"PostToolUse","tool_response":"{}"}}"#,
            "x".repeat(6 * 1024 * 1024)
        );
        for (pane, token) in [("", ""), ("", "guess")] {
            let raw = request(port, pane, token, "", &big);
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s.write_all(&raw).expect("whole body written");
            let mut out = String::new();
            s.read_to_string(&mut out).expect("clean close, no reset");
            let want = if token.is_empty() { 204 } else { 403 };
            assert_eq!(out, response(want));
        }
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    /// Claude sends hook N+1 only after N's response; the registry must see
    /// them in that order even when applying one is slow.
    #[test]
    fn listener_applies_events_in_arrival_order() {
        let listener = bind_loopback(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let tx = parking_lot::Mutex::new(tx);
        serve(
            listener,
            TOKEN.into(),
            Arc::new(ours),
            Arc::new(move |ev: HookEvent| {
                // Earlier events are slower to apply.
                let n: u64 = ev.session_id.parse().unwrap();
                std::thread::sleep(Duration::from_millis((5 - n) * 40));
                let _ = tx.lock().send(n);
            }),
        )
        .unwrap();
        for n in 0..5 {
            let body = format!(r#"{{"hook_event_name":"PostToolUse","session_id":"{n}"}}"#);
            let resp = send(port, &request(port, &pane().to_string(), TOKEN, "", &body));
            assert_eq!(resp, response(204));
        }
        let got: Vec<u64> = (0..5)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn listener_reads_a_body_split_across_packets() {
        let (port, rx) = start();
        let body = r#"{"hook_event_name":"Stop","session_id":"s2"}"#;
        let raw = request(port, &pane().to_string(), TOKEN, "", body);
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let cut = raw.len() - 10;
        s.write_all(&raw[..cut]).unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        s.write_all(&raw[cut..]).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert_eq!(out, response(204));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().event,
            "Stop"
        );
    }
}
