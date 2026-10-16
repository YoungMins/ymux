//! Summarizer transport: executes the `HttpRequest`s the provider modules
//! build, lists models, and runs the turn-end summary with a fallback to the
//! rule summary on ANY error. The HTTP executor is injected (tests use a
//! fake; the desktop build supplies `http_executor`).
//!
//! Safety: a request to a non-loopback host is refused unless `allow_remote`
//! is set, so facts and the API key never leave the machine by accident.
//! Error values carry a kind only (no URL, no body).

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use super::rule::Summary;
use super::{is_loopback_resolved, ollama, openai, HttpRequest};
use crate::config::model::SummarizerConfig;

/// Response bytes read from a model server.
pub const MAX_RESPONSE_BYTES: u64 = 256 * 1024;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Jobs waiting behind the one in flight.
pub const QUEUE_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// Non-loopback endpoint without `allow_remote`, or not http(s).
    Refused,
    Connect,
    Timeout,
    /// Non-2xx.
    Status(u16),
    TooLarge,
    /// Unusable reply, unknown provider or an empty summary.
    Parse,
}

/// Runs one request with a total timeout and returns the 2xx body.
pub type Executor =
    Arc<dyn Fn(&HttpRequest, Duration) -> Result<String, TransportError> + Send + Sync>;

fn timeout_of(cfg: &SummarizerConfig) -> Duration {
    Duration::from_secs(u64::from(cfg.timeout_secs.clamp(1, 300)))
}

fn guard(url: &str, allow_remote: bool) -> Result<(), TransportError> {
    // Loopback is judged on what the host *resolves to*, not how it is spelled.
    if is_loopback_resolved(url) {
        return Ok(());
    }
    // Remote is fine only when allowed, and only over http(s).
    let ok_scheme = url::Url::parse(url.trim())
        .map(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
        .unwrap_or(false);
    if allow_remote && ok_scheme {
        Ok(())
    } else {
        Err(TransportError::Refused)
    }
}

fn run(
    exec: &Executor,
    req: &HttpRequest,
    allow_remote: bool,
    timeout: Duration,
) -> Result<String, TransportError> {
    guard(&req.url, allow_remote)?;
    exec(req, timeout)
}

/// Models offered by `provider` (`ollama` | `openai`) at `endpoint`.
pub fn list_models(
    exec: &Executor,
    provider: &str,
    endpoint: &str,
    api_key: &str,
    allow_remote: bool,
    timeout: Duration,
) -> Result<Vec<String>, TransportError> {
    let (req, parse): (HttpRequest, fn(&str) -> Vec<String>) = match provider {
        "ollama" => (ollama::list_models_request(endpoint), ollama::parse_models),
        "openai" => (
            openai::list_models_request(endpoint, api_key),
            openai::parse_models,
        ),
        _ => return Err(TransportError::Parse),
    };
    let body = run(exec, &req, allow_remote, timeout)?;
    Ok(parse(&body))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryResult {
    pub summary: Summary,
    /// False when the rule summary was used (provider `rule` or any error).
    pub used_llm: bool,
}

fn llm_text(
    exec: &Executor,
    cfg: &SummarizerConfig,
    api_key: &str,
    lang: &str,
    facts_json: &str,
) -> Result<String, TransportError> {
    let max = (cfg.max_input_chars as usize).max(1);
    let facts: String = facts_json.chars().take(max).collect();
    let timeout = timeout_of(cfg);
    let (req, parse): (HttpRequest, fn(&str) -> Option<String>) = match cfg.provider.as_str() {
        "ollama" => (
            ollama::chat_request(&cfg.endpoint, &cfg.model, lang, &facts),
            ollama::parse_chat,
        ),
        "openai" => (
            openai::chat_request(&cfg.endpoint, api_key, &cfg.model, lang, &facts),
            openai::parse_chat,
        ),
        _ => return Err(TransportError::Parse),
    };
    if cfg.model.trim().is_empty() {
        return Err(TransportError::Parse);
    }
    let body = run(exec, &req, cfg.allow_remote, timeout)?;
    parse(&body).ok_or(TransportError::Parse)
}

/// The model's text goes in as a bullet under the rule headline, so the
/// factual headline survives a hallucinating or chatty model. Any error
/// returns `fallback` unchanged.
pub fn summarize(
    exec: &Executor,
    cfg: &SummarizerConfig,
    api_key: &str,
    lang: &str,
    facts_json: &str,
    fallback: &Summary,
) -> SummaryResult {
    match llm_text(exec, cfg, api_key, lang, facts_json) {
        Ok(text) => {
            let mut s = Summary {
                headline: fallback.headline.clone(),
                bullets: vec![text],
            };
            s.bullets.extend(fallback.bullets.iter().take(2).cloned());
            SummaryResult {
                summary: s,
                used_llm: true,
            }
        }
        Err(_) => SummaryResult {
            summary: fallback.clone(),
            used_llm: false,
        },
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
                .user_agent(concat!("ymux/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
}

/// The real executor: 2 s connect, caller's total timeout, no redirects,
/// response read capped at `MAX_RESPONSE_BYTES`.
#[cfg(feature = "desktop")]
pub fn http_executor() -> Executor {
    Arc::new(|req, timeout| {
        use std::io::Read;
        let Some(client) = client() else {
            return Err(TransportError::Connect);
        };
        let mut rb = match req.method {
            "POST" => client.post(&req.url),
            _ => client.get(&req.url),
        }
        .timeout(timeout);
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(b) = &req.body {
            rb = rb.body(b.clone());
        }
        let mut resp = rb.send().map_err(|e| {
            // The URL (a user-supplied endpoint) stays out of the log line.
            let e = e.without_url();
            tracing::debug!(error = %e, "summarizer request failed");
            if e.is_timeout() {
                TransportError::Timeout
            } else {
                TransportError::Connect
            }
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(TransportError::Status(status));
        }
        let mut buf = Vec::new();
        (&mut resp)
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    TransportError::Timeout
                } else {
                    TransportError::Connect
                }
            })?;
        if buf.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(TransportError::TooLarge);
        }
        String::from_utf8(buf).map_err(|_| TransportError::Parse)
    })
}

// ------------------------------------------------------------------ worker

/// One summary to produce. `done` runs on the worker thread.
pub struct Job {
    pub facts_json: String,
    pub fallback: Summary,
    pub lang: String,
    pub done: Box<dyn FnOnce(SummaryResult) + Send>,
}

/// Current provider settings and API key, read at the start of every job.
pub type SettingsFn = Arc<dyn Fn() -> (SummarizerConfig, String) + Send + Sync>;

/// One in flight, `QUEUE_CAP` waiting. The thread ends when the `Summarizer`
/// is dropped.
pub struct Summarizer {
    tx: SyncSender<Job>,
}

impl Summarizer {
    pub fn start(exec: Executor, settings: SettingsFn) -> Option<Summarizer> {
        let (tx, rx) = sync_channel::<Job>(QUEUE_CAP);
        std::thread::Builder::new()
            .name("summarizer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let (cfg, key) = settings();
                    let res =
                        summarize(&exec, &cfg, &key, &job.lang, &job.facts_json, &job.fallback);
                    // A panicking callback must not kill the worker.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (job.done)(res);
                    }));
                }
            })
            .ok()?;
        Some(Summarizer { tx })
    }

    /// False when the queue is full (or the worker is gone): the caller keeps
    /// the rule summary. Never blocks.
    pub fn submit(&self, job: Job) -> bool {
        match self.tx.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;

    fn cfg(provider: &str) -> SummarizerConfig {
        SummarizerConfig {
            provider: provider.into(),
            endpoint: "http://127.0.0.1:11434".into(),
            model: "gemma3:1b".into(),
            ..SummarizerConfig::default()
        }
    }

    fn fallback() -> Summary {
        Summary {
            headline: "Changed 2 files (+1/-1)".into(),
            bullets: vec!["tests passed".into()],
        }
    }

    fn fake(
        reply: Result<&'static str, TransportError>,
    ) -> (Executor, Arc<Mutex<Vec<HttpRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let e: Executor = Arc::new(move |req, _| {
            s.lock().push(req.clone());
            reply.map(str::to_string)
        });
        (e, seen)
    }

    #[test]
    fn ollama_summary_replaces_nothing_but_adds_a_bullet() {
        let (e, seen) = fake(Ok(r#"{"message":{"content":"Edited two files."}}"#));
        let r = summarize(&e, &cfg("ollama"), "", "en", "{\"f\":1}", &fallback());
        assert!(r.used_llm);
        assert_eq!(r.summary.headline, "Changed 2 files (+1/-1)");
        assert_eq!(r.summary.bullets[0], "Edited two files.");
        assert_eq!(r.summary.bullets[1], "tests passed");
        assert_eq!(seen.lock()[0].url, "http://127.0.0.1:11434/api/chat");
    }

    #[test]
    fn openai_summary_sends_the_key_to_loopback() {
        let (e, seen) = fake(Ok(r#"{"choices":[{"message":{"content":"Done."}}]}"#));
        let mut c = cfg("openai");
        c.endpoint = "http://localhost:1234".into();
        let r = summarize(&e, &c, "sk-test", "en", "{}", &fallback());
        assert!(r.used_llm);
        let req = seen.lock()[0].clone();
        assert!(req.url.ends_with("/v1/chat/completions"));
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
    }

    #[test]
    fn every_failure_falls_back_to_the_rule_summary() {
        for reply in [
            Err(TransportError::Timeout),
            Err(TransportError::Connect),
            Err(TransportError::Status(500)),
            Err(TransportError::TooLarge),
            Ok("not json"),
            Ok(r#"{"message":{"content":"  "}}"#),
        ] {
            let (e, _) = fake(reply);
            let r = summarize(&e, &cfg("ollama"), "", "en", "{}", &fallback());
            assert!(!r.used_llm, "{reply:?}");
            assert_eq!(r.summary, fallback());
        }
    }

    #[test]
    fn rule_provider_unknown_provider_and_missing_model_never_call_out() {
        let (e, seen) = fake(Ok("{}"));
        for provider in ["rule", "", "mystery"] {
            let r = summarize(&e, &cfg(provider), "", "en", "{}", &fallback());
            assert!(!r.used_llm);
        }
        let mut c = cfg("ollama");
        c.model = "  ".into();
        assert!(!summarize(&e, &c, "", "en", "{}", &fallback()).used_llm);
        assert!(seen.lock().is_empty());
    }

    #[test]
    fn non_loopback_is_refused_unless_allowed_and_nothing_is_sent() {
        let (e, seen) = fake(Ok(r#"{"choices":[{"message":{"content":"Done."}}]}"#));
        let mut c = cfg("openai");
        c.endpoint = "https://api.example.com".into();
        let r = summarize(&e, &c, "sk-secret", "en", "{}", &fallback());
        assert!(!r.used_llm);
        assert!(seen.lock().is_empty(), "no request, so no key leaked");
        c.allow_remote = true;
        assert!(summarize(&e, &c, "sk-secret", "en", "{}", &fallback()).used_llm);
        assert_eq!(seen.lock().len(), 1);
        // allow_remote does not open other schemes.
        c.endpoint = "ftp://example.com".into();
        assert!(!summarize(&e, &c, "", "en", "{}", &fallback()).used_llm);
        assert_eq!(seen.lock().len(), 1);
    }

    #[test]
    fn facts_are_clipped_to_max_input_chars() {
        let (e, seen) = fake(Ok(r#"{"message":{"content":"ok"}}"#));
        let mut c = cfg("ollama");
        c.max_input_chars = 10;
        summarize(&e, &c, "", "en", &"x".repeat(500), &fallback());
        let body: serde_json::Value =
            serde_json::from_str(seen.lock()[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["messages"][1]["content"].as_str().unwrap().len(), 10);
    }

    #[test]
    fn list_models_per_provider() {
        let (e, seen) = fake(Ok(
            r#"{"models":[{"name":"gemma3:1b"},{"name":"qwen3:4b"}]}"#,
        ));
        let m = list_models(
            &e,
            "ollama",
            "http://127.0.0.1:11434",
            "",
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m, Ok(vec!["gemma3:1b".to_string(), "qwen3:4b".to_string()]));
        assert_eq!(seen.lock()[0].url, "http://127.0.0.1:11434/api/tags");

        let (e, _) = fake(Ok(r#"{"data":[{"id":"m1"}]}"#));
        let m = list_models(
            &e,
            "openai",
            "http://localhost:1234/v1",
            "k",
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m, Ok(vec!["m1".to_string()]));

        let m = list_models(
            &e,
            "openai",
            "https://x.example.com",
            "k",
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m, Err(TransportError::Refused));
        let m = list_models(
            &e,
            "rule",
            "http://127.0.0.1",
            "",
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m, Err(TransportError::Parse));
        let (e, _) = fake(Err(TransportError::Status(401)));
        let m = list_models(
            &e,
            "openai",
            "http://localhost",
            "k",
            false,
            Duration::from_secs(2),
        );
        assert_eq!(m, Err(TransportError::Status(401)));
    }

    #[test]
    fn timeout_is_taken_from_config_and_clamped() {
        let mut c = cfg("ollama");
        c.timeout_secs = 0;
        assert_eq!(timeout_of(&c), Duration::from_secs(1));
        c.timeout_secs = 20;
        assert_eq!(timeout_of(&c), Duration::from_secs(20));
        c.timeout_secs = u32::MAX;
        assert_eq!(timeout_of(&c), Duration::from_secs(300));
        let got = Arc::new(Mutex::new(Duration::ZERO));
        let g = got.clone();
        let e: Executor = Arc::new(move |_, t| {
            *g.lock() = t;
            Ok(r#"{"message":{"content":"ok"}}"#.to_string())
        });
        c.timeout_secs = 7;
        summarize(&e, &c, "", "en", "{}", &fallback());
        assert_eq!(*got.lock(), Duration::from_secs(7));
    }

    #[test]
    fn one_in_flight_and_a_queue_of_four() {
        let gate = Arc::new(AtomicBool::new(false));
        let g = gate.clone();
        let started = Arc::new(AtomicBool::new(false));
        let st = started.clone();
        let e: Executor = Arc::new(move |_, _| {
            st.store(true, Ordering::SeqCst);
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(r#"{"message":{"content":"ok"}}"#.to_string())
        });
        let s = Summarizer::start(e, Arc::new(|| (cfg("ollama"), String::new()))).unwrap();
        let (tx, rx) = channel();
        let job = |i: usize| {
            let tx = tx.clone();
            Job {
                facts_json: "{}".into(),
                fallback: fallback(),
                lang: "en".into(),
                done: Box::new(move |r| {
                    let _ = tx.send((i, r.used_llm));
                }),
            }
        };
        assert!(s.submit(job(0)));
        let t = std::time::Instant::now();
        while !started.load(Ordering::SeqCst) {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        for i in 1..=QUEUE_CAP {
            assert!(s.submit(job(i)), "slot {i}");
        }
        assert!(!s.submit(job(99)), "queue full");
        gate.store(true, Ordering::SeqCst);
        let mut done = Vec::new();
        for _ in 0..=QUEUE_CAP {
            done.push(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        }
        assert_eq!(
            done.iter().map(|d| d.0).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4],
            "in order"
        );
        assert!(done.iter().all(|d| d.1));
    }

    #[test]
    fn a_panicking_callback_does_not_kill_the_worker() {
        let (e, _) = fake(Ok(r#"{"message":{"content":"ok"}}"#));
        let s = Summarizer::start(e, Arc::new(|| (cfg("ollama"), String::new()))).unwrap();
        let mk = |f: Box<dyn FnOnce(SummaryResult) + Send>| Job {
            facts_json: "{}".into(),
            fallback: fallback(),
            lang: "en".into(),
            done: f,
        };
        assert!(s.submit(mk(Box::new(|_| panic!("callback bug")))));
        let (tx, rx) = channel();
        let t = std::time::Instant::now();
        loop {
            let tx = tx.clone();
            if s.submit(mk(Box::new(move |r| {
                let _ = tx.send(r.used_llm);
            }))) {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(true));
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn real_executor_reads_a_local_reply_and_maps_status() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        fn serve(status_line: &'static str, body: &'static str) -> u16 {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            std::thread::spawn(move || {
                if let Ok((mut s, _)) = l.accept() {
                    let mut b = [0u8; 4096];
                    let _ = s.read(&mut b);
                    let _ = write!(
                        s,
                        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            });
            port
        }
        let e = http_executor();
        let req = |port: u16| HttpRequest {
            method: "GET",
            url: format!("http://127.0.0.1:{port}/api/tags"),
            headers: vec![],
            body: None,
        };
        let p = serve("200 OK", r#"{"models":[]}"#);
        assert_eq!(
            e(&req(p), Duration::from_secs(5)),
            Ok(r#"{"models":[]}"#.to_string())
        );
        let p = serve("503 Service Unavailable", "no");
        assert_eq!(
            e(&req(p), Duration::from_secs(5)),
            Err(TransportError::Status(503))
        );
    }
}
