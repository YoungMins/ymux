//! Remote server link (CLAUDE.md rule 19): an outbound WebSocket to the
//! user's own relay, through which a paired phone can list agent panes, read a
//! pane's screen, answer a menu, send Escape and send a prompt.
//!
//! The desktop is the only place a command is authorised. Everything that
//! decides anything is pure and tested on Linux CI: `proto` (shapes, signed
//! strings), `keys` (P-256, pairing proof), `session` (the section 5.1
//! authorisation), `pairing`, `clip` (redaction + clipping of every outgoing
//! text), `panes` (notification transitions, send throttle), `engine` (the
//! protocol state machine over those), `devset` (the revision gate and the
//! server-switch order), `backoff`, `audit`. `runtime` (desktop
//! only) owns the socket thread and the Tauri commands.

pub mod audit;
pub mod backoff;
pub mod clip;
pub mod devset;
pub mod engine;
pub mod keys;
pub mod pairing;
pub mod panes;
pub mod proto;
pub mod session;

/// Vectors produced by the yMux-Server repo (`fixtures/`), checked against
/// this side's encoders, signer and authoriser.
#[cfg(test)]
mod fixture_tests;

#[cfg(feature = "desktop")]
pub mod runtime;

/// Env var that lets a debug build connect (a `tauri dev` build shares the
/// live config with the installed app, so it must not claim the desktop id).
pub const DEV_ENV: &str = "YMUX_DEV_REMOTE";

/// Path appended to the server's base URL for the desktop socket.
const WS_PATH: &str = "/ws/desktop";

/// May this process hold the connection at all? Never in a debug build
/// without the opt-in, never in a stand-down instance (the one without a hook
/// receiver: another ymux owns the machine's agent state).
pub fn connection_allowed(debug_build: bool, dev_opt_in: Option<&str>, standdown: bool) -> bool {
    if standdown {
        return false;
    }
    !debug_build || dev_opt_in.map(str::trim) == Some("1")
}

/// Why a server URL was refused. Short codes, safe to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    Invalid,
    Scheme,
    PlainHttpNotLocal,
    Credentials,
    QueryOrFragment,
}

impl UrlError {
    pub fn message(self) -> &'static str {
        match self {
            UrlError::Empty => "the server address is empty",
            UrlError::Invalid => "that is not a valid web address",
            UrlError::Scheme => "the address must start with https://",
            UrlError::PlainHttpNotLocal => {
                "http:// is only allowed for localhost and 127.0.0.1; use https://"
            }
            UrlError::Credentials => "the address must not contain a user name or password",
            UrlError::QueryOrFragment => "the address must not contain ? or #",
        }
    }
}

/// Canonical `https://host[:port][/base]` (no trailing slash) or the reason it
/// is not acceptable. Plain `http://` is accepted for `localhost` and
/// `127.0.0.1` only.
pub fn normalize_server_url(raw: &str) -> Result<String, UrlError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(UrlError::Empty);
    }
    let u = url::Url::parse(raw).map_err(|_| UrlError::Invalid)?;
    let host = u
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or(UrlError::Invalid)?;
    match u.scheme() {
        "https" => {}
        "http" => {
            if host != "localhost" && host != "127.0.0.1" {
                return Err(UrlError::PlainHttpNotLocal);
            }
        }
        _ => return Err(UrlError::Scheme),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(UrlError::Credentials);
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err(UrlError::QueryOrFragment);
    }
    let mut out = format!("{}://{}", u.scheme(), host);
    if let Some(p) = u.port() {
        out.push_str(&format!(":{p}"));
    }
    out.push_str(u.path().trim_end_matches('/'));
    Ok(out)
}

/// The origin of a server address, `scheme://host[:port]`: lowercase, default
/// ports left out, no path (what a browser's `location.origin` gives). The
/// server names this in its challenge and the hello is signed for it.
pub fn server_origin(raw: &str) -> Result<String, UrlError> {
    let norm = normalize_server_url(raw)?;
    let u = url::Url::parse(&norm).map_err(|_| UrlError::Invalid)?;
    let host = u.host_str().ok_or(UrlError::Invalid)?;
    let mut out = format!("{}://{}", u.scheme(), host);
    if let Some(p) = u.port() {
        out.push_str(&format!(":{p}"));
    }
    Ok(out)
}

/// The desktop socket URL for a normalized server URL.
pub fn ws_url(server_url: &str) -> Result<String, UrlError> {
    let base = normalize_server_url(server_url)?;
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .ok_or(UrlError::Scheme)?;
    Ok(format!("{rest}{WS_PATH}"))
}

/// What the QR code carries: `<server>/#pair=<desktop_id>.<code>.<dk>`, `dk`
/// being this window's public key-agreement key. The fragment never reaches an
/// HTTP request.
pub fn pair_link(server_url: &str, desktop_id: &str, code_b64u: &str, dk_b64u: &str) -> String {
    format!(
        "{}/#pair={desktop_id}.{code_b64u}.{dk_b64u}",
        server_url.trim_end_matches('/')
    )
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_gate() {
        assert!(connection_allowed(false, None, false));
        assert!(!connection_allowed(true, None, false));
        assert!(!connection_allowed(true, Some("0"), false));
        assert!(!connection_allowed(true, Some(""), false));
        assert!(connection_allowed(true, Some("1"), false));
        // A stand-down instance never connects, opt-in or not.
        assert!(!connection_allowed(false, None, true));
        assert!(!connection_allowed(true, Some("1"), true));
    }

    #[test]
    fn urls_normalise_and_map_to_the_socket() {
        assert_eq!(
            normalize_server_url(" https://r.example.com/ ").unwrap(),
            "https://r.example.com"
        );
        assert_eq!(
            normalize_server_url("https://r.example.com:8443/ymux/").unwrap(),
            "https://r.example.com:8443/ymux"
        );
        assert_eq!(
            ws_url("https://r.example.com:8443/ymux").unwrap(),
            "wss://r.example.com:8443/ymux/ws/desktop"
        );
        assert_eq!(
            ws_url("https://r.example.com").unwrap(),
            "wss://r.example.com/ws/desktop"
        );
        assert_eq!(
            ws_url("http://localhost:8080").unwrap(),
            "ws://localhost:8080/ws/desktop"
        );
        assert_eq!(
            ws_url("http://127.0.0.1:9").unwrap(),
            "ws://127.0.0.1:9/ws/desktop"
        );
        // Default ports are dropped by the parser.
        assert_eq!(
            normalize_server_url("https://a.example:443").unwrap(),
            "https://a.example"
        );
    }

    #[test]
    fn origin_is_lowercase_without_default_port_or_path() {
        for (raw, want) in [
            ("https://r.example.com", "https://r.example.com"),
            ("https://R.Example.COM/", "https://r.example.com"),
            ("HTTPS://r.example.com:443/ymux/", "https://r.example.com"),
            (
                "https://r.example.com:8443/ymux",
                "https://r.example.com:8443",
            ),
            ("http://localhost:8080/x", "http://localhost:8080"),
            ("http://127.0.0.1:80", "http://127.0.0.1"),
            (" https://r.example.com:443 ", "https://r.example.com"),
        ] {
            assert_eq!(server_origin(raw).as_deref(), Ok(want), "{raw}");
        }
        assert_eq!(server_origin(""), Err(UrlError::Empty));
        assert_eq!(
            server_origin("http://evil.example"),
            Err(UrlError::PlainHttpNotLocal)
        );
    }

    #[test]
    fn bad_urls_are_refused() {
        use UrlError::*;
        let e = |s: &str| normalize_server_url(s).unwrap_err();
        assert_eq!(e(""), Empty);
        assert_eq!(e("   "), Empty);
        assert_eq!(e("nonsense"), Invalid);
        assert_eq!(e("ftp://r.example.com"), Scheme);
        assert_eq!(e("ws://r.example.com"), Scheme);
        assert_eq!(e("wss://r.example.com"), Scheme);
        assert_eq!(e("http://r.example.com"), PlainHttpNotLocal);
        assert_eq!(e("http://192.168.1.5"), PlainHttpNotLocal);
        assert_eq!(e("http://[::1]:3000"), PlainHttpNotLocal);
        assert_eq!(e("https://u:p@r.example.com"), Credentials);
        assert_eq!(e("https://u@r.example.com"), Credentials);
        assert_eq!(e("https://r.example.com/?x=1"), QueryOrFragment);
        assert_eq!(e("https://r.example.com/#pair=1"), QueryOrFragment);
        assert!(ws_url("http://evil.example").is_err());
    }

    #[test]
    fn pair_link_has_the_fragment_form() {
        assert_eq!(
            pair_link("https://r.example.com/x", "dddd", "CODE", "DK"),
            "https://r.example.com/x/#pair=dddd.CODE.DK"
        );
        // A real one stays well within a QR code's capacity (version 11, M).
        let ek = keys::EcdhKey::generate().public_b64u();
        let link = pair_link(
            "https://ymux.example.com",
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            &keys::random_b64u(16),
            &ek,
        );
        assert!(link.len() < 230, "{}", link.len());
    }
}
