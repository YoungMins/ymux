//! Webhook delivery, pure part: destination model, payload builders, HMAC
//! signing, retry/backoff policy and a persisted per-destination queue.
//!
//! Nothing here touches the network. The delivery worker (desktop) feeds these
//! functions and owns the HTTP client.

pub mod backoff;
pub mod client;
pub mod delivery;
pub mod hmac;
pub mod payload;
pub mod queue;
#[cfg(feature = "desktop")]
pub mod runtime;

use serde::{Deserialize, Serialize};

/// A destination id is a UUID (hyphenated or simple, lowercase). It names the
/// destination's queue file and secrets entry, so anything else is refused
/// rather than sanitized: two different ids must never share a file.
pub fn is_valid_dest_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id)
        .is_ok_and(|u| id == u.hyphenated().to_string() || id == u.simple().to_string())
}

/// One configured webhook destination. The URL and HMAC secret are NOT here:
/// they live in the backend-owned secrets store (`url_hint` is a redacted
/// display form only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookDestination {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// `json` | `slack` | `discord` | `teams`.
    pub format: String,
    pub url_hint: String,
    /// Event kinds delivered: `turn_end`, `attention`.
    pub events: Vec<String>,
    /// `summary` | `full`.
    pub detail: String,
    pub include_prompts: bool,
    pub debounce_secs: u32,
    pub attach: bool,
    pub attach_types: Vec<String>,
    pub max_file_mb: u32,
    pub max_total_mb: u32,
    pub max_files: u32,
}

impl WebhookDestination {
    /// Whether `id` may name a queue and a secrets entry (see
    /// [`is_valid_dest_id`]). A destination without one is never delivered to.
    pub fn has_valid_id(&self) -> bool {
        is_valid_dest_id(&self.id)
    }
}

impl Default for WebhookDestination {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: false,
            format: "json".to_string(),
            url_hint: String::new(),
            events: vec!["turn_end".to_string(), "attention".to_string()],
            detail: "summary".to_string(),
            include_prompts: false,
            debounce_secs: 10,
            attach: false,
            attach_types: Vec::new(),
            max_file_mb: 8,
            max_total_mb: 8,
            max_files: 4,
        }
    }
}

/// Per-file change counts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub add: u32,
    pub del: u32,
}

/// Test run outcome seen during the turn.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TestInfo {
    pub runner: String,
    pub passed: Option<bool>,
}

/// Self-contained input of the payload builders. Every string is untrusted.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DeliveryEvent {
    /// `turn_end` | `attention`.
    pub kind: String,
    /// Unix milliseconds.
    pub ts: i64,
    pub ws_name: String,
    pub pane_title: String,
    pub agent: String,
    pub account_name: String,
    pub branch: String,
    pub cwd_display: String,
    pub headline: String,
    pub bullets: Vec<String>,
    pub files: Vec<FileStat>,
    pub tests: Option<TestInfo>,
    pub duration_ms: Option<u64>,
    pub prompt: Option<String>,
    /// Backend-side context for the attachment planner. Never serialized
    /// (not in the queue file, not in a payload): the queue stores the
    /// planner's verdict, not its input.
    #[serde(skip)]
    pub plan: Option<PlanHint>,
}

/// Where a turn ran and when it began, from backend state (never a hook
/// claim). The planner derives the repo root from `cwd`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanHint {
    pub cwd: String,
    pub turn_start_ms: u64,
    /// When the turn ended (Unix ms); 0 = unknown (no upper mtime bound).
    pub turn_end_ms: u64,
}

/// Most destinations kept.
pub const MAX_DESTINATIONS: usize = 20;
/// Attachment caps a destination may ask for (the Settings UI mirrors these;
/// `webhookLimits.test.ts` keeps the two in sync).
pub const MAX_ATTACH_FILE_MB: u32 = 25;
pub const MAX_ATTACH_TOTAL_MB: u32 = 25;
pub const MAX_ATTACH_FILES: u32 = 10;

/// `scheme://host` of a webhook URL for display; empty when it is not an
/// http(s) URL. Userinfo, port, path and query (where tokens live) are left
/// out.
pub fn url_hint(url: &str) -> String {
    match url::Url::parse(url.trim()) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u
            .host_str()
            .map(|h| format!("{}://{h}", u.scheme()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// What `save_config` accepts from the frontend: invalid and duplicate ids
/// are dropped, enums and numbers are clamped, and `url_hint` is NEVER taken
/// from `incoming` (it is kept from `existing` for the same id, else empty;
/// the backend rewrites it when the URL secret changes).
pub fn sanitize_destinations(
    incoming: Vec<WebhookDestination>,
    existing: &[WebhookDestination],
) -> Vec<WebhookDestination> {
    let mut out: Vec<WebhookDestination> = Vec::new();
    for mut d in incoming {
        if !d.has_valid_id() || out.iter().any(|o| o.id == d.id) {
            continue;
        }
        if out.len() >= MAX_DESTINATIONS {
            break;
        }
        d.url_hint = existing
            .iter()
            .find(|e| e.id == d.id)
            .map(|e| e.url_hint.clone())
            .unwrap_or_default();
        if !matches!(d.format.as_str(), "json" | "slack" | "discord" | "teams") {
            d.format = "json".to_string();
        }
        if d.detail != "full" {
            d.detail = "summary".to_string();
        }
        let mut seen: Vec<String> = Vec::new();
        d.events.retain(|k| {
            let ok = (k == "turn_end" || k == "attention") && !seen.contains(k);
            if ok {
                seen.push(k.clone());
            }
            ok
        });
        d.name = d.name.chars().take(100).collect();
        d.debounce_secs = d.debounce_secs.min(3600);
        d.max_file_mb = d.max_file_mb.clamp(1, MAX_ATTACH_FILE_MB);
        d.max_total_mb = d.max_total_mb.clamp(1, MAX_ATTACH_TOTAL_MB);
        d.max_files = d.max_files.min(MAX_ATTACH_FILES);
        d.attach_types = d
            .attach_types
            .iter()
            .map(|t| t.trim().trim_start_matches('.').to_lowercase())
            .filter(|t| {
                !t.is_empty() && t.len() <= 8 && t.chars().all(|c| c.is_ascii_alphanumeric())
            })
            .take(20)
            .collect();
        out.push(d);
    }
    out
}

/// A file that will travel with the message (multipart) or be linked.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub path: String,
    /// Identity of the approved file (device + inode, or volume serial + file
    /// index) and its mtime in Unix ms; `attach::open_verified` re-checks
    /// them on the handle it opens at send time.
    #[serde(default)]
    pub dev: u64,
    #[serde(default)]
    pub ino: u64,
    #[serde(default)]
    pub mtime_ms: u64,
}

/// A file that is only listed, with the reason it was not attached.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ListedFile {
    pub name: String,
    pub size: u64,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_uuids_are_destination_ids() {
        let u = uuid::Uuid::new_v4();
        assert!(is_valid_dest_id(&u.hyphenated().to_string()));
        assert!(is_valid_dest_id(&u.simple().to_string()));
        for bad in [
            "",
            "a",
            "../../evil/name",
            &u.hyphenated().to_string().to_uppercase(),
            &format!("{{{u}}}"),
            &format!("urn:uuid:{u}"),
            &format!("{u} "),
            "0123456789abcdef0123456789abcdeg",
        ] {
            assert!(!is_valid_dest_id(bad), "{bad:?}");
        }
        let d = WebhookDestination {
            id: u.to_string(),
            ..Default::default()
        };
        assert!(d.has_valid_id());
        assert!(!WebhookDestination::default().has_valid_id());
    }

    #[test]
    fn url_hint_keeps_scheme_and_host_only() {
        assert_eq!(
            url_hint("https://user:pw@hooks.slack.com:8443/services/T0/B0/SECRET?x=1"),
            "https://hooks.slack.com"
        );
        assert_eq!(url_hint(" http://127.0.0.1:9/h "), "http://127.0.0.1");
        for bad in [
            "",
            "not a url",
            "ftp://host/x",
            "file:///etc/passwd",
            "javascript:1",
        ] {
            assert_eq!(url_hint(bad), "", "{bad}");
        }
    }

    #[test]
    fn sanitize_drops_bad_ids_and_never_trusts_the_hint() {
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        let existing = vec![WebhookDestination {
            id: a.clone(),
            url_hint: "https://real.example".into(),
            ..Default::default()
        }];
        let dest = |id: &str| WebhookDestination {
            id: id.to_string(),
            url_hint: "https://forged.example/SECRET".into(),
            format: "bogus".into(),
            detail: "x".into(),
            events: vec!["turn_end".into(), "nope".into()],
            max_file_mb: 9999,
            max_files: 9999,
            attach_types: vec![".PNG".into(), "../x".into(), " pdf ".into()],
            ..Default::default()
        };
        let out = sanitize_destinations(
            vec![dest(&a), dest(&a), dest(&b), dest("evil/../id"), dest("")],
            &existing,
        );
        assert_eq!(
            out.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            [&a, &b]
        );
        assert_eq!(out[0].url_hint, "https://real.example");
        assert_eq!(out[1].url_hint, "");
        assert_eq!(out[0].format, "json");
        assert_eq!(out[0].detail, "summary");
        assert_eq!(out[0].events, vec!["turn_end"]);
        assert_eq!((out[0].max_file_mb, out[0].max_files), (25, 10));
        assert_eq!(out[0].attach_types, vec!["png", "pdf"]);
    }

    #[test]
    fn sanitize_dedupes_events_keeping_first_order() {
        let d = WebhookDestination {
            id: uuid::Uuid::new_v4().to_string(),
            events: vec![
                "attention".into(),
                "turn_end".into(),
                "attention".into(),
                "turn_end".into(),
            ],
            ..Default::default()
        };
        let out = sanitize_destinations(vec![d], &[]);
        assert_eq!(out[0].events, vec!["attention", "turn_end"]);
    }

    #[test]
    fn destination_defaults_and_partial_toml() {
        let d: WebhookDestination = toml::from_str("id = \"a\"\nformat = \"slack\"").unwrap();
        assert_eq!(d.id, "a");
        assert_eq!(d.format, "slack");
        assert!(!d.enabled);
        assert_eq!(d.events, vec!["turn_end", "attention"]);
        assert_eq!(d.detail, "summary");
        assert_eq!(d.debounce_secs, 10);
        assert_eq!((d.max_file_mb, d.max_total_mb, d.max_files), (8, 8, 4));
    }

    #[test]
    fn destination_toml_roundtrip() {
        let d = WebhookDestination {
            id: "x".into(),
            attach: true,
            attach_types: vec!["png".into(), "pdf".into()],
            ..Default::default()
        };
        let s = toml::to_string_pretty(&d).unwrap();
        let back: WebhookDestination = toml::from_str(&s).unwrap();
        assert_eq!(d, back);
    }
}
