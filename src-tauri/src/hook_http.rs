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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::agents::HookEvent;

/// The one path the receiver answers on. Also the ownership marker of every
/// hook entry `agent_hooks` installs (CLAUDE.md rule 12).
pub const HOOK_PATH: &str = "/ymux-agent-hook";
/// Unauthenticated liveness check: `GET` here answers [`PING_BODY`] and
/// nothing else, so a second ymux can tell "another ymux has the port" from
/// "a stranger has the port" (see [`choose_port`]).
pub const PING_PATH: &str = "/ymux-agent-hook/ping";
/// The fixed body of a ping answer. Identifies ymux; carries no state.
pub const PING_BODY: &str = "ymux-agent-hook/1";
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
/// Resource limits of the receiver. Any local user can connect (the token is
/// only checked once the head has arrived), so every connection is bounded
/// in time as a whole and in number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Whole-connection budget, from accept to close — reading the head and
    /// body, writing the reply and any drain. Not per read: a client
    /// trickling a byte a second still runs out. Claude sends its whole
    /// request at once over loopback; anything slower is not Claude.
    pub deadline: Duration,
    /// Connections served at once; more are closed on accept.
    pub max_conns: usize,
}

/// The limits the app runs with.
pub const LIMITS: Limits = Limits {
    deadline: Duration::from_secs(3),
    max_conns: 32,
};

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
    /// Answer [`ping_response`].
    Ping,
    /// Answer this non-2xx status with an empty body.
    Reject(u16),
}

/// The auth decision (CLAUDE.md rule 13). In order:
/// 1. an `Origin` header → 403. Browsers attach it to every cross-origin
///    POST; Claude Code's client never sends one;
/// 2. a `Host` that isn't loopback → 403 (DNS-rebinding belt and braces);
/// 3. `GET` [`PING_PATH`] → the ping answer, no token needed;
/// 4. not `POST` → 405, not [`HOOK_PATH`] → 404;
/// 5. an empty token header → 204, ignored: that is a Claude session started
///    outside ymux (its env has neither variable), or in a pane of a ymux
///    running without a receiver, and a non-2xx would put a hook error in
///    it. An empty token authorises nothing, so ignoring it is safe;
/// 6. wrong token (constant-time compare), or a pane that isn't a UUID or
///    isn't one of ours → 403;
/// 7. no `Content-Length` or any `Transfer-Encoding` → 411;
/// 8. body over [`MAX_BODY`] → 204, dropped (see there).
pub fn authorize(head: &RequestHead, token: &str, known_pane: impl Fn(Uuid) -> bool) -> Verdict {
    if head.header("origin").is_some() {
        return Verdict::Reject(403);
    }
    if let Some(host) = head.header("host") {
        if !host_is_loopback(host) {
            return Verdict::Reject(403);
        }
    }
    if head.method == "GET" && head.path == PING_PATH {
        return Verdict::Ping;
    }
    if head.method != "POST" && head.path == HOOK_PATH {
        return Verdict::Reject(405);
    }
    if head.path != HOOK_PATH {
        return Verdict::Reject(404);
    }
    let pane = head.header(PANE_HEADER).unwrap_or("");
    let sent = head.header(TOKEN_HEADER).unwrap_or("");
    if sent.is_empty() {
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

/// The ping answer: 200 with [`PING_BODY`]. Never sent to Claude (it only
/// POSTs), so a non-empty body is fine here.
pub fn ping_response() -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{PING_BODY}",
        PING_BODY.len()
    )
}

/// Outcome of [`choose_port`].
#[derive(Debug, PartialEq, Eq)]
pub enum Bound<L> {
    /// Listening; `reused` = on the persisted port. `false` means the
    /// caller should persist the new port and refresh the installed hooks.
    Listening { listener: L, reused: bool },
    /// The persisted port is held by another running ymux. Run without a
    /// receiver and leave `settings.json` alone: moving the port would
    /// strand that instance's panes, sending their token to a port anyone
    /// could take next.
    HeldByYmux,
}

/// Bind the receiver: the persisted port when it is set and free; when it is
/// taken, stand down if `is_ymux` says another ymux holds it, else take a
/// fresh OS-assigned port. Generic over `bind` / `is_ymux` so the decision is
/// tested without depending on which ports happen to be free.
pub fn choose_port<L>(
    persisted: u16,
    mut bind: impl FnMut(u16) -> io::Result<L>,
    is_ymux: impl FnOnce(u16) -> bool,
) -> io::Result<Bound<L>> {
    if persisted != 0 {
        match bind(persisted) {
            Ok(listener) => {
                return Ok(Bound::Listening {
                    listener,
                    reused: true,
                })
            }
            Err(_) if is_ymux(persisted) => return Ok(Bound::HeldByYmux),
            Err(_) => {}
        }
    }
    bind(0).map(|listener| Bound::Listening {
        listener,
        reused: false,
    })
}

/// Whether a ymux hook receiver answers on `127.0.0.1:port` (a `GET`
/// [`PING_PATH`] whose reply is exactly [`ping_response`]). Bounded to about
/// a second. A stranger faking the answer only makes this ymux run without
/// hook events — no token is ever sent to it.
pub fn probe_ymux(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(700);
    let req =
        format!("GET {PING_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.set_write_timeout(Some(Duration::from_millis(300)))
        .is_err()
        || s.write_all(req.as_bytes()).is_err()
    {
        return false;
    }
    let want = ping_response();
    let mut got = Vec::with_capacity(want.len());
    let mut chunk = [0u8; 256];
    while got.len() <= want.len() {
        match read_by(&mut s, &mut chunk, deadline) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
        }
    }
    got == want.as_bytes()
}

/// Listen on `127.0.0.1:port` — loopback only, never `0.0.0.0`.
///
/// On Windows the socket takes the port with `SO_EXCLUSIVEADDRUSE`: without
/// it, another process binding the same address with `SO_REUSEADDR` could
/// share the port and receive hook bodies meant for ymux. Unix needs nothing
/// extra — std sets only `SO_REUSEADDR`, which never lets a second socket
/// bind a port that is listening, and `SO_REUSEPORT` sharing needs the owner
/// to opt in too.
pub fn bind_loopback(port: u16) -> io::Result<TcpListener> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    #[cfg(windows)]
    {
        bind_exclusive(addr)
    }
    #[cfg(not(windows))]
    {
        TcpListener::bind(addr)
    }
}

#[cfg(windows)]
fn bind_exclusive(addr: SocketAddr) -> io::Result<TcpListener> {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_REUSEADDR};
    // `SO_EXCLUSIVEADDRUSE` is `((int)(~SO_REUSEADDR))` in winsock2.h; the
    // `windows` crate's metadata doesn't carry it.
    const SO_EXCLUSIVEADDRUSE: i32 = !SO_REUSEADDR;
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    let on = 1i32.to_ne_bytes();
    // SAFETY: a live socket handle owned by `socket`, and a 4-byte BOOL.
    let rc = unsafe {
        setsockopt(
            SOCKET(socket.as_raw_socket() as usize),
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            Some(&on),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

pub type KnownPane = Arc<dyn Fn(Uuid) -> bool + Send + Sync>;
pub type OnEvent = Arc<dyn Fn(HookEvent) + Send + Sync>;

/// One of at most `max` concurrently served connections; frees its slot on
/// drop.
#[derive(Debug)]
pub struct ConnSlot(Arc<AtomicUsize>);

impl ConnSlot {
    /// A slot, or `None` when `max` are already taken.
    pub fn acquire(count: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        let prev = count.fetch_add(1, Ordering::SeqCst);
        if prev >= max {
            count.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(count.clone()))
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Time left before `deadline`, or `None` once it has passed.
pub fn remaining(deadline: Instant, now: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|d| !d.is_zero())
}

/// One `read` that may not outlast `deadline`: the socket's read timeout is
/// set to the time left before every read, so a slow client can never
/// stretch the connection past it.
fn read_by(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
    let left = remaining(deadline, Instant::now()).ok_or(io::ErrorKind::TimedOut)?;
    stream.set_read_timeout(Some(left))?;
    stream.read(buf)
}

/// Run the accept loop on a background thread for the life of the process,
/// with the app's [`LIMITS`].
pub fn serve(
    listener: TcpListener,
    token: String,
    known_pane: KnownPane,
    on_event: OnEvent,
) -> io::Result<std::thread::JoinHandle<()>> {
    serve_with(listener, token, known_pane, on_event, LIMITS)
}

/// [`serve`] with explicit limits.
/// Each connection is served on its own short-lived thread; accepted events
/// go through one channel to a single applier thread that calls `on_event`.
///
/// Order matters: Claude Code sends hook N+1 only after hook N's response,
/// so queueing each event *before* its response keeps the registry's order
/// equal to Claude's (a `Stop` can't overtake the last `PostToolUse`), while
/// the response still never waits for `on_event` and its locks.
pub fn serve_with(
    listener: TcpListener,
    token: String,
    known_pane: KnownPane,
    on_event: OnEvent,
    limits: Limits,
) -> io::Result<std::thread::JoinHandle<()>> {
    let token: Arc<str> = token.into();
    let live = Arc::new(AtomicUsize::new(0));
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
                // Over the cap: dropping the stream closes it at once.
                let Some(slot) = ConnSlot::acquire(&live, limits.max_conns) else {
                    continue;
                };
                let deadline = Instant::now() + limits.deadline;
                let (token, known_pane, tx) = (token.clone(), known_pane.clone(), tx.clone());
                let _ = std::thread::Builder::new()
                    .name("ymux-hook-conn".into())
                    .spawn(move || {
                        let _slot = slot;
                        let _ = handle(stream, &token, &*known_pane, &tx, deadline);
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
    deadline: Instant,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return reply(&mut stream, 431, deadline);
        }
        let n = read_by(&mut stream, &mut chunk, deadline)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let Some(head) = parse_head(&buf[..head_end]) else {
        return reply(&mut stream, 400, deadline);
    };
    let (pane, len) = match authorize(&head, token, known_pane) {
        Verdict::Accept { pane, len } => (pane, len),
        Verdict::Ignore => {
            reply(&mut stream, 204, deadline)?;
            // Only a quiet 204 is worth a drain: it goes to a legitimate
            // Claude that may still be uploading (see `drain`). A rejected
            // client gets its status and the door, never our time.
            drain(&mut stream, deadline);
            return Ok(());
        }
        Verdict::Ping => {
            let left = remaining(deadline, Instant::now()).ok_or(io::ErrorKind::TimedOut)?;
            stream.set_write_timeout(Some(left))?;
            stream.write_all(ping_response().as_bytes())?;
            let _ = stream.shutdown(std::net::Shutdown::Write);
            return Ok(());
        }
        Verdict::Reject(status) => return reply(&mut stream, status, deadline),
    };
    // `head_end + 4 <= buf.len()`: the blank line was found inside `buf`.
    let mut body = buf.split_off(head_end + 4);
    if body.len() < len {
        let mut have = body.len();
        body.resize(len, 0);
        while have < len {
            let n = read_by(&mut stream, &mut body[have..], deadline)?;
            if n == 0 {
                return Ok(());
            }
            have += n;
        }
    }
    body.truncate(len);
    let Some(event) = hook_event(&body, pane) else {
        return reply(&mut stream, 400, deadline);
    };
    let _ = queue.send(event);
    reply(&mut stream, 204, deadline)
}

/// Upper bound on request bytes discarded after an early reply.
const MAX_DRAIN: u64 = MAX_BODY as u64 + MAX_HEAD as u64;

/// Write `status` (within the deadline) and half-close.
fn reply(stream: &mut TcpStream, status: u16, deadline: Instant) -> io::Result<()> {
    let left = remaining(deadline, Instant::now()).ok_or(io::ErrorKind::TimedOut)?;
    stream.set_write_timeout(Some(left))?;
    stream.write_all(response(status).as_bytes())?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

/// Read and discard what the client is still sending, bounded by
/// [`MAX_DRAIN`] and the deadline. Closing a socket with unread bytes queued
/// makes the OS send a reset instead of a clean close, and a client still
/// uploading a large body — a Claude outside ymux posting a big
/// `PostToolUse` — would see that as a failed hook.
fn drain(stream: &mut TcpStream, deadline: Instant) {
    let mut chunk = [0u8; 8192];
    let mut left = MAX_DRAIN;
    while left > 0 {
        match read_by(stream, &mut chunk, deadline) {
            Ok(0) | Err(_) => return,
            Ok(n) => left = left.saturating_sub(n as u64),
        }
    }
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
        for bad in ["nope", &TOKEN[..31]] {
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

    /// A pane of a ymux that runs without a receiver (another instance
    /// holds the port) has an id but an empty token: quiet, not an error.
    #[test]
    fn a_pane_without_a_token_is_ignored_quietly() {
        let head = claude_head(&[(TOKEN_HEADER, "")]);
        assert_eq!(authorize(&head, TOKEN, ours), Verdict::Ignore);
    }

    fn ping_head(extra: &[(&str, &str)]) -> RequestHead {
        let mut headers: Vec<(String, String)> = vec![("host".into(), "127.0.0.1:41234".into())];
        headers.extend(extra.iter().map(|(k, v)| ((*k).into(), (*v).into())));
        RequestHead {
            method: "GET".into(),
            path: PING_PATH.into(),
            headers,
        }
    }

    #[test]
    fn ping_needs_no_token_but_refuses_browsers() {
        assert_eq!(authorize(&ping_head(&[]), TOKEN, ours), Verdict::Ping);
        assert_eq!(
            authorize(
                &ping_head(&[("origin", "https://evil.example")]),
                TOKEN,
                ours
            ),
            Verdict::Reject(403)
        );
        let mut post = ping_head(&[]);
        post.method = "POST".into();
        assert_eq!(authorize(&post, TOKEN, ours), Verdict::Reject(404));
        let mut get_hook = ping_head(&[]);
        get_hook.path = HOOK_PATH.into();
        assert_eq!(authorize(&get_hook, TOKEN, ours), Verdict::Reject(405));
        let r = ping_response();
        assert!(r.starts_with("HTTP/1.1 200 "));
        assert!(r.ends_with(&format!("\r\n\r\n{PING_BODY}")));
    }

    #[test]
    fn choose_port_reuses_the_persisted_port_when_free() {
        let mut tried = Vec::new();
        let got = choose_port(
            41234,
            |p| {
                tried.push(p);
                Ok(p)
            },
            |_| panic!("no probe when the bind worked"),
        )
        .unwrap();
        assert_eq!(
            got,
            Bound::Listening {
                listener: 41234,
                reused: true
            }
        );
        assert_eq!(tried, vec![41234]);
    }

    fn busy(p: u16) -> io::Result<u16> {
        if p == 0 {
            Ok(50000)
        } else {
            Err(io::ErrorKind::AddrInUse.into())
        }
    }

    #[test]
    fn choose_port_moves_only_off_a_foreign_holder() {
        assert_eq!(
            choose_port(41234, busy, |p| {
                assert_eq!(p, 41234);
                false
            })
            .unwrap(),
            Bound::Listening {
                listener: 50000,
                reused: false
            }
        );
        // A live ymux holds it: stay off it, never move the hooks.
        assert_eq!(
            choose_port(41234, busy, |_| true).unwrap(),
            Bound::HeldByYmux
        );
        // First run: nothing persisted, straight to a fresh port.
        assert_eq!(
            choose_port(0, busy, |_| panic!("nothing to probe")).unwrap(),
            Bound::Listening {
                listener: 50000,
                reused: false
            }
        );
    }

    #[test]
    fn choose_port_with_real_sockets() {
        // A foreign holder (not ymux): move to a fresh port.
        let taken = bind_loopback(0).unwrap();
        let foreign = taken.local_addr().unwrap().port();
        let Bound::Listening { listener, reused } =
            choose_port(foreign, bind_loopback, probe_ymux).unwrap()
        else {
            panic!("a foreign holder must not look like ymux");
        };
        assert!(!reused);
        let fresh = listener.local_addr().unwrap();
        assert_ne!(fresh.port(), foreign);
        assert!(fresh.ip().is_loopback());
        drop(listener);
        drop(taken);
        let Bound::Listening { listener, reused } =
            choose_port(foreign, bind_loopback, probe_ymux).unwrap()
        else {
            panic!("free port");
        };
        assert!(reused);
        assert_eq!(listener.local_addr().unwrap().port(), foreign);

        // A live ymux receiver holds it: leave it alone.
        let (port, _rx) = start();
        assert!(probe_ymux(port));
        assert!(matches!(
            choose_port(port, bind_loopback, probe_ymux).unwrap(),
            Bound::HeldByYmux
        ));
    }

    #[test]
    fn probe_is_false_for_nothing_and_for_strangers() {
        let l = bind_loopback(0).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(!probe_ymux(port), "nothing listening");
        // A server that answers something else.
        let l = bind_loopback(0).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = l.accept() {
                let mut b = [0u8; 512];
                let _ = c.read(&mut b);
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
            }
        });
        assert!(!probe_ymux(port));
    }

    /// While ymux listens, nobody else gets its connections: a same-address
    /// `SO_REUSEADDR` squat is refused, and a wildcard (`0.0.0.0:port`)
    /// squatter — which Windows does let bind — still loses every
    /// connection to `127.0.0.1:port` to the more specific socket. (Measured
    /// on Windows 11: `SO_EXCLUSIVEADDRUSE` changes neither outcome there —
    /// it is defence in depth for stacks without enhanced socket security.)
    #[cfg(windows)]
    #[test]
    fn a_squatter_cannot_take_connections_while_ymux_listens() {
        use socket2::{Domain, Socket, Type};
        let ours = bind_loopback(0).unwrap();
        let port = ours.local_addr().unwrap().port();
        let same = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        same.set_reuse_address(true).unwrap();
        assert!(same.bind(&ours.local_addr().unwrap().into()).is_err());

        let wild = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        wild.set_reuse_address(true).unwrap();
        let wild_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
        if wild.bind(&wild_addr.into()).is_ok() && wild.listen(8).is_ok() {
            wild.set_nonblocking(true).unwrap();
            let _c = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            ours.set_nonblocking(false).unwrap();
            let (_s, _) = ours.accept().expect("ymux gets the connection");
            assert!(wild.accept().is_err(), "the squatter got nothing");
        }
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
    fn listener_answers_ping() {
        let (port, _rx) = start();
        let raw = format!("GET {PING_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        assert_eq!(send(port, raw.as_bytes()), ping_response());
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
        // Only the quiet 204 is drained. A 403 closes on the unread body on
        // purpose (`a_rejected_request_is_not_drained`), so a big upload to
        // it may well end in a reset: Linux reports that as EPIPE on the
        // write, while Windows' loopback buffers happened to absorb it.
        let raw = request(port, "", "", "", &big);
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(&raw).expect("whole body written");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("clean close, no reset");
        assert_eq!(out, response(204));
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
    fn conn_slots_are_capped_and_freed() {
        let count = Arc::new(AtomicUsize::new(0));
        let a = ConnSlot::acquire(&count, 2).unwrap();
        let _b = ConnSlot::acquire(&count, 2).unwrap();
        assert!(ConnSlot::acquire(&count, 2).is_none());
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "a refused acquire frees itself"
        );
        drop(a);
        assert!(ConnSlot::acquire(&count, 2).is_some());
    }

    #[test]
    fn remaining_runs_out() {
        let now = Instant::now();
        let later = now + Duration::from_millis(5);
        assert_eq!(remaining(later, now), Some(Duration::from_millis(5)));
        assert_eq!(remaining(now, now), None);
        assert_eq!(remaining(now, later), None);
    }

    fn start_with(limits: Limits) -> (u16, mpsc::Receiver<HookEvent>) {
        let listener = bind_loopback(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let tx = parking_lot::Mutex::new(tx);
        serve_with(
            listener,
            TOKEN.into(),
            Arc::new(ours),
            Arc::new(move |ev| {
                let _ = tx.lock().send(ev);
            }),
            limits,
        )
        .unwrap();
        (port, rx)
    }

    /// Read until the server closes (or 10 s pass); how long that took.
    fn time_to_close(s: &mut TcpStream) -> Duration {
        let t = Instant::now();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink);
        t.elapsed()
    }

    /// A client trickling one byte at a time — each read well inside any
    /// per-read timeout — is still cut off at the whole-request deadline.
    #[test]
    fn a_trickling_client_is_cut_off_at_the_deadline() {
        let (port, _rx) = start_with(Limits {
            deadline: Duration::from_millis(600),
            max_conns: 4,
        });
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let mut w = s.try_clone().unwrap();
        let t = Instant::now();
        let trickle = std::thread::spawn(move || {
            for b in b"POST /ymux-agent-hook HTTP/1.1\r\nX-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaa" {
                if w.write_all(&[*b]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let _ = time_to_close(&mut s);
        assert!(
            t.elapsed() < Duration::from_millis(2500),
            "{:?}",
            t.elapsed()
        );
        let _ = trickle.join();
    }

    #[test]
    fn connections_over_the_cap_are_closed_at_once() {
        let limits = Limits {
            deadline: Duration::from_secs(5),
            max_conns: 2,
        };
        let (port, rx) = start_with(limits);
        let idle: Vec<TcpStream> = (0..2)
            .map(|_| TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap())
            .collect();
        std::thread::sleep(Duration::from_millis(100));
        let mut extra = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        assert!(time_to_close(&mut extra) < Duration::from_secs(2));
        drop(idle);
        std::thread::sleep(Duration::from_millis(100));
        let body = r#"{"hook_event_name":"Stop"}"#;
        let resp = send(port, &request(port, &pane().to_string(), TOKEN, "", body));
        assert_eq!(resp, response(204), "slots come back");
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    /// A rejected request is answered and closed without waiting for the
    /// body it announced.
    #[test]
    fn a_rejected_request_is_not_drained() {
        let (port, _rx) = start_with(Limits {
            deadline: Duration::from_secs(5),
            max_conns: 4,
        });
        let head = format!(
            "POST {HOOK_PATH} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             X-Ymux-Pane: {}\r\nX-Ymux-Token: guess\r\nContent-Length: 8000000\r\n\r\n",
            pane()
        );
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(&[b'x'; 1000]).unwrap();
        assert!(time_to_close(&mut s) < Duration::from_secs(2));
    }

    /// Tiny xorshift so the fuzz cases are reproducible without a new
    /// dependency.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Hostile input must never panic: the release profile aborts on panic,
    /// which would take the whole app down.
    #[test]
    fn hostile_heads_and_bodies_never_panic() {
        let alphabet: &[u8] = b"POST /ymux-agent-hook HTTP/1.1\r\n:Content-Length:X-Ymux-Token \
                                Transfer-Encoding Origin Host 127.0.0.1 -+0123456789\xff\x00{}\"";
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..20_000 {
            let len = (rng(&mut seed) % 200) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| alphabet[(rng(&mut seed) % alphabet.len() as u64) as usize])
                .collect();
            if let Some(head) = parse_head(&bytes) {
                let _ = authorize(&head, TOKEN, ours);
            }
            let _ = hook_event(&bytes, pane());
        }
        for cl in [
            "-1",
            "",
            " ",
            "+5",
            "0x10",
            "99999999999999999999999999",
            "18446744073709551615",
            "1e9",
            "5, 5",
        ] {
            let head = claude_head(&[("content-length", cl)]);
            assert!(!matches!(
                authorize(&head, TOKEN, ours),
                Verdict::Accept { len, .. } if len > MAX_BODY
            ));
        }
    }

    /// The same hostile framings over a real socket: each gets an answer or
    /// a close, and the listener keeps serving afterwards.
    #[test]
    fn listener_survives_hostile_requests() {
        let (port, rx) = start_with(Limits {
            deadline: Duration::from_millis(800),
            max_conns: 8,
        });
        let p = pane().to_string();
        let raws: Vec<Vec<u8>> = vec![
            b"\r\n\r\n".to_vec(),
            b"POST\r\n\r\n".to_vec(),
            b"GET / HTTP/1.1\r\n\xff\xfe: x\r\n\r\n".to_vec(),
            format!("POST {HOOK_PATH} HTTP/1.1\r\nX-Ymux-Pane: {p}\r\nX-Ymux-Token: {TOKEN}\r\nContent-Length: -1\r\n\r\n").into_bytes(),
            format!("POST {HOOK_PATH} HTTP/1.1\r\nX-Ymux-Pane: {p}\r\nX-Ymux-Token: {TOKEN}\r\nContent-Length: 99999999999999999999\r\n\r\n").into_bytes(),
            format!("POST {HOOK_PATH} HTTP/1.1\r\nX-Ymux-Pane: {p}\r\nX-Ymux-Token: {TOKEN}\r\nContent-Length: 50\r\n\r\nshort").into_bytes(),
            format!("POST {HOOK_PATH} HTTP/1.1\r\nX-Ymux-Pane: {p}\r\nX-Ymux-Token: {TOKEN}\r\nContent-Length: 4\r\n\r\n").bytes().chain([0xff, 0xfe, 0, b'{']).collect(),
        ];
        for raw in raws {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            let _ = s.write_all(&raw);
            let _ = s.shutdown(std::net::Shutdown::Write);
            assert!(time_to_close(&mut s) < Duration::from_secs(3));
        }
        let body = r#"{"hook_event_name":"Stop"}"#;
        assert_eq!(
            send(port, &request(port, &p, TOKEN, "", body)),
            response(204)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().event,
            "Stop"
        );
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
