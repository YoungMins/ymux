//! Webhook HTTP client. One shared `reqwest::blocking::Client`: no redirects,
//! 5 s connect, 30 s total, http/https only, request body <= 25 MiB, response
//! read <= 64 KiB. Errors are reported as a kind only: the URL (which is a
//! secret) never reaches a log line or an error string.
//!
//! The outcome types and URL checks are pure; only `send` needs `reqwest`
//! (desktop).

use std::time::Duration;

#[cfg(feature = "desktop")]
use super::payload::Built;

pub const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a send failed before any HTTP status was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Not an `http(s)` URL with a host.
    Scheme,
    /// Body over `MAX_BODY_BYTES`.
    TooLarge,
    /// Client or request could not be built.
    Build,
    Connect,
    Timeout,
    Request,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Scheme => "scheme",
            ErrorKind::TooLarge => "too_large",
            ErrorKind::Build => "build",
            ErrorKind::Connect => "connect",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Request => "request",
        }
    }

    /// Whether trying again could help.
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorKind::Connect | ErrorKind::Timeout | ErrorKind::Request
        )
    }
}

/// Result of one send. `status` is `None` when no response arrived.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SendOutcome {
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
    pub error_kind: Option<ErrorKind>,
}

impl SendOutcome {
    pub fn status(code: u16) -> Self {
        Self {
            status: Some(code),
            ..Self::default()
        }
    }

    pub fn failed(kind: ErrorKind) -> Self {
        Self {
            error_kind: Some(kind),
            ..Self::default()
        }
    }
}

/// `http`/`https` with a host; anything else (file:, ftp:, data:, ...) is
/// refused.
pub fn check_url(url: &str) -> Result<(), ErrorKind> {
    match url::Url::parse(url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.host().is_some() => Ok(()),
        _ => Err(ErrorKind::Scheme),
    }
}

/// True for `http://` to a non-loopback host: the payload would cross the
/// network unencrypted. The UI warns; sending is still allowed.
pub fn warn_insecure(url: &str) -> bool {
    match url::Url::parse(url.trim()) {
        Ok(u) => u.scheme() == "http" && !crate::summarizer::is_loopback_endpoint(url),
        Err(_) => false,
    }
}

#[cfg(feature = "desktop")]
fn client() -> Option<&'static reqwest::blocking::Client> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(TOTAL_TIMEOUT)
                .user_agent(concat!("ymux/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
}

/// POSTs `built` to `url`. Never panics, never returns the URL.
#[cfg(feature = "desktop")]
pub fn send(url: &str, built: &Built) -> SendOutcome {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
    use std::io::Read;

    if let Err(k) = check_url(url) {
        return SendOutcome::failed(k);
    }
    if built.body.len() > MAX_BODY_BYTES {
        return SendOutcome::failed(ErrorKind::TooLarge);
    }
    let Some(client) = client() else {
        return SendOutcome::failed(ErrorKind::Build);
    };
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&built.content_type) {
        headers.insert(CONTENT_TYPE, v);
    }
    for (k, v) in &built.headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            if name != CONTENT_TYPE && name != reqwest::header::HOST {
                headers.insert(name, val);
            }
        }
    }
    let resp = client
        .post(url.trim())
        .headers(headers)
        .body(built.body.clone())
        .send();
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            // reqwest errors embed the URL, which is the secret.
            let e = e.without_url();
            tracing::debug!(error = %e, "webhook request failed");
            let kind = if e.is_timeout() {
                ErrorKind::Timeout
            } else if e.is_connect() {
                ErrorKind::Connect
            } else if e.is_builder() {
                ErrorKind::Build
            } else {
                ErrorKind::Request
            };
            return SendOutcome::failed(kind);
        }
    };
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(super::backoff::parse_retry_after);
    // Drain a bounded amount so the connection ends cleanly; the body itself
    // is never used.
    let mut sink = Vec::new();
    let _ = (&mut resp).take(MAX_RESPONSE_BYTES).read_to_end(&mut sink);
    SendOutcome {
        status: Some(status),
        retry_after,
        error_kind: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_with_a_host() {
        assert!(check_url("https://hooks.example.com/x").is_ok());
        assert!(check_url(" http://127.0.0.1:8080/x ").is_ok());
        for bad in [
            "file:///etc/passwd",
            "ftp://x/y",
            "data:text/plain,hi",
            "javascript:alert(1)",
            "hooks.example.com/x",
            "https://",
            "",
        ] {
            assert_eq!(check_url(bad), Err(ErrorKind::Scheme), "{bad}");
        }
    }

    #[test]
    fn insecure_means_plain_http_off_loopback() {
        assert!(warn_insecure("http://hooks.example.com/x"));
        assert!(warn_insecure("http://10.0.0.5/x"));
        assert!(!warn_insecure("https://hooks.example.com/x"));
        assert!(!warn_insecure("http://localhost:3000/x"));
        assert!(!warn_insecure("http://127.0.0.1/x"));
        assert!(!warn_insecure("http://[::1]/x"));
        assert!(!warn_insecure("garbage"));
    }

    #[test]
    fn error_kinds_have_no_url_and_retry_rules() {
        assert!(ErrorKind::Timeout.retryable());
        assert!(ErrorKind::Connect.retryable());
        assert!(!ErrorKind::Scheme.retryable());
        assert!(!ErrorKind::TooLarge.retryable());
        assert_eq!(ErrorKind::TooLarge.as_str(), "too_large");
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn send_refuses_bad_scheme_and_oversized_body_without_network() {
        let b = Built {
            content_type: "application/json".into(),
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert_eq!(
            send("file:///x", &b),
            SendOutcome::failed(ErrorKind::Scheme)
        );
        let big = Built {
            body: vec![0; MAX_BODY_BYTES + 1],
            ..b
        };
        assert_eq!(
            send("https://example.invalid/x", &big),
            SendOutcome::failed(ErrorKind::TooLarge)
        );
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn send_against_a_local_listener_reads_status_and_retry_after() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let b = Built {
            content_type: "application/json".into(),
            headers: vec![("X-Test".into(), "1".into())],
            body: b"{}".to_vec(),
        };
        let o = send(&format!("http://127.0.0.1:{port}/h"), &b);
        let _ = t.join();
        assert_eq!(o.status, Some(429));
        assert_eq!(o.retry_after, Some(Duration::from_secs(7)));
        assert_eq!(o.error_kind, None);
    }
}
