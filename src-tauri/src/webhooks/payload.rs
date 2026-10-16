//! Payload builders: events -> (content type, headers, body bytes) per
//! destination format. Pure; the caller injects ids, time, secret and the
//! multipart boundary. All event text is untrusted: it is cleaned of control
//! and bidi characters and clipped to each platform's limits.

use serde_json::{json, Map, Value};

use super::hmac;
use super::{AttachmentRef, DeliveryEvent, ListedFile, WebhookDestination};

/// Events per message; callers chunk longer batches.
pub const MAX_BATCH: usize = 10;

/// Injected inputs that make the builders deterministic.
pub struct BuildCtx<'a> {
    pub delivery_id: &'a str,
    pub ts_secs: i64,
    /// HMAC secret for the `json` format; empty = unsigned.
    pub secret: &'a str,
    pub boundary: &'a str,
}

/// A file with its bytes, ready for a multipart part.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub meta: AttachmentRef,
    pub data: Vec<u8>,
}

/// A finished HTTP request body.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One multipart/form-data part.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------- text utils

fn is_bad(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' | '\u{FEFF}')
}

/// Removes control/bidi characters. Newlines survive only with `keep_nl`;
/// otherwise controls become spaces. Trims.
pub fn clean(s: &str, keep_nl: bool) -> String {
    let mapped: String = s
        .chars()
        .filter_map(|c| {
            if c == '\n' && keep_nl {
                Some('\n')
            } else if c == '\r' || c == '\t' || c == '\n' {
                Some(' ')
            } else if is_bad(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    mapped.trim().to_string()
}

/// At most `max` chars, ending in an ellipsis when cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('\u{2026}');
    out
}

fn line(s: &str, max: usize) -> String {
    clip(&clean(s, false), max)
}

fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else {
        format!("{:.1} MB", f / (K * K))
    }
}

fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// RFC 3339 UTC from unix milliseconds (pure civil-date arithmetic).
pub fn iso8601(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// ------------------------------------------------------------------ multipart

fn safe_boundary(b: &str) -> String {
    let s: String = b
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(70)
        .collect();
    if s.is_empty() {
        "ymux-boundary".to_string()
    } else {
        s
    }
}

fn header_token(s: &str) -> String {
    clean(s, false)
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '/'))
        .take(120)
        .collect()
}

/// Encodes parts as multipart/form-data. Returns the `Content-Type` value and
/// the body. The boundary is sanitised; the caller must make it unguessable.
pub fn encode_multipart(boundary: &str, parts: &[Part]) -> (String, Vec<u8>) {
    let b = safe_boundary(boundary);
    let mut out: Vec<u8> = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{b}\r\n").as_bytes());
        let mut disp = format!(
            "Content-Disposition: form-data; name=\"{}\"",
            header_token(&p.name)
        );
        if let Some(f) = &p.filename {
            let f = header_token(f);
            disp.push_str(&format!(
                "; filename=\"{}\"",
                if f.is_empty() { "file" } else { &f }
            ));
        }
        out.extend_from_slice(disp.as_bytes());
        out.extend_from_slice(b"\r\n");
        if let Some(ct) = &p.content_type {
            let ct = clean(ct, false);
            out.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{b}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={b}"), out)
}

// ----------------------------------------------------------------- shared view

struct View {
    title: String,
    bullets: Vec<String>,
    facts: Vec<(String, String)>,
    ts: i64,
    attention: bool,
    failed: bool,
}

fn kind_label(kind: &str) -> String {
    match kind {
        "turn_end" => "Turn finished".to_string(),
        "attention" => "Needs attention".to_string(),
        other => line(other, 40),
    }
}

fn full(dest: &WebhookDestination) -> bool {
    dest.detail == "full"
}

fn view(dest: &WebhookDestination, e: &DeliveryEvent) -> View {
    let headline = line(&e.headline, 200);
    let title = if headline.is_empty() {
        kind_label(&e.kind)
    } else {
        format!("{} \u{b7} {}", kind_label(&e.kind), headline)
    };
    let bullets: Vec<String> = e
        .bullets
        .iter()
        .map(|b| line(b, 300))
        .filter(|b| !b.is_empty())
        .take(8)
        .collect();
    let mut facts: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, v: String| {
        if !v.is_empty() {
            facts.push((k.to_string(), v));
        }
    };
    add("Workspace", line(&e.ws_name, 100));
    add("Agent", line(&e.agent, 60));
    add("Branch", line(&e.branch, 100));
    let mut failed = false;
    if let Some(t) = &e.tests {
        failed = t.passed == Some(false);
    }
    if full(dest) {
        add("Pane", line(&e.pane_title, 100));
        add("Account", line(&e.account_name, 60));
        add("Directory", line(&e.cwd_display, 200));
        if let Some(ms) = e.duration_ms {
            add("Duration", fmt_duration(ms));
        }
        if let Some(t) = &e.tests {
            let verdict = match t.passed {
                Some(true) => "passed",
                Some(false) => "failed",
                None => "ran",
            };
            add(
                "Tests",
                format!("{} {}", line(&t.runner, 40), verdict)
                    .trim()
                    .to_string(),
            );
        }
        if !e.files.is_empty() {
            let mut s = e
                .files
                .iter()
                .take(10)
                .map(|f| format!("{} +{}/-{}", line(&f.path, 120), f.add, f.del))
                .collect::<Vec<_>>()
                .join("\n");
            if e.files.len() > 10 {
                s.push_str(&format!("\n+{} more", e.files.len() - 10));
            }
            add("Files", s);
        }
        if dest.include_prompts {
            if let Some(p) = &e.prompt {
                add("Prompt", clip(&clean(p, true), 500));
            }
        }
    }
    View {
        title,
        bullets,
        facts,
        ts: e.ts,
        attention: e.kind == "attention",
        failed,
    }
}

fn selected<'a>(dest: &WebhookDestination, events: &'a [DeliveryEvent]) -> Vec<&'a DeliveryEvent> {
    events
        .iter()
        .filter(|e| dest.events.iter().any(|k| k == &e.kind))
        .take(MAX_BATCH)
        .collect()
}

fn file_lines(attached: &[Attachment], listed: &[ListedFile], link_only: bool) -> Vec<String> {
    let mut out = Vec::new();
    for a in attached {
        let note = if link_only { " (not uploaded)" } else { "" };
        out.push(format!(
            "{} ({}){}",
            line(&a.meta.name, 120),
            human_size(a.meta.size),
            note
        ));
    }
    for l in listed {
        let reason = line(&l.reason, 80);
        let tail = if reason.is_empty() {
            String::new()
        } else {
            format!(" \u{2014} {reason}")
        };
        out.push(format!(
            "{} ({}){}",
            line(&l.name, 120),
            human_size(l.size),
            tail
        ));
    }
    out
}

// ------------------------------------------------------------------- builders

/// Builds one message for `dest`. Returns `None` when no event passes the
/// destination's event filter. At most `MAX_BATCH` events are used. Attachments
/// are honoured only when `dest.attach`; where the format cannot carry files
/// (slack, teams) or the count exceeds the format's limit, they are listed.
pub fn build(
    dest: &WebhookDestination,
    ctx: &BuildCtx<'_>,
    events: &[DeliveryEvent],
    attached: &[Attachment],
    listed: &[ListedFile],
) -> Option<Built> {
    let evs = selected(dest, events);
    if evs.is_empty() {
        return None;
    }
    let views: Vec<View> = evs.iter().map(|e| view(dest, e)).collect();
    let no_files: &[Attachment] = &[];
    let attached = if dest.attach { attached } else { no_files };
    Some(match dest.format.as_str() {
        "slack" => build_slack(&views, attached, listed),
        "discord" => build_discord(&views, ctx, attached, listed),
        "teams" => build_teams(&views, attached, listed),
        _ => build_json(dest, ctx, &evs, attached, listed),
    })
}

fn build_json(
    dest: &WebhookDestination,
    ctx: &BuildCtx<'_>,
    evs: &[&DeliveryEvent],
    attached: &[Attachment],
    listed: &[ListedFile],
) -> Built {
    let events: Vec<Value> = evs
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("kind".into(), json!(line(&e.kind, 40)));
            m.insert("ts".into(), json!(e.ts));
            m.insert("workspace".into(), json!(line(&e.ws_name, 100)));
            m.insert("agent".into(), json!(line(&e.agent, 60)));
            m.insert("branch".into(), json!(line(&e.branch, 100)));
            m.insert("headline".into(), json!(line(&e.headline, 200)));
            m.insert(
                "bullets".into(),
                json!(e
                    .bullets
                    .iter()
                    .map(|b| line(b, 300))
                    .take(8)
                    .collect::<Vec<_>>()),
            );
            if full(dest) {
                m.insert("pane".into(), json!(line(&e.pane_title, 100)));
                m.insert("account".into(), json!(line(&e.account_name, 60)));
                m.insert("cwd".into(), json!(line(&e.cwd_display, 200)));
                m.insert(
                    "files".into(),
                    json!(e
                        .files
                        .iter()
                        .take(50)
                        .map(|f| json!({"path": line(&f.path, 200), "add": f.add, "del": f.del}))
                        .collect::<Vec<_>>()),
                );
                if let Some(t) = &e.tests {
                    m.insert(
                        "tests".into(),
                        json!({"runner": line(&t.runner, 40), "passed": t.passed}),
                    );
                }
                if let Some(ms) = e.duration_ms {
                    m.insert("duration_ms".into(), json!(ms));
                }
                if dest.include_prompts {
                    if let Some(p) = &e.prompt {
                        m.insert("prompt".into(), json!(clip(&clean(p, true), 500)));
                    }
                }
            }
            Value::Object(m)
        })
        .collect();
    let mut root = Map::new();
    root.insert("v".into(), json!(1));
    root.insert("source".into(), json!("ymux"));
    root.insert("events".into(), Value::Array(events));
    if !attached.is_empty() {
        root.insert(
            "attachments".into(),
            json!(attached
                .iter()
                .enumerate()
                .map(|(i, a)| json!({
                    "part": format!("file{}", i + 1),
                    "name": line(&a.meta.name, 120),
                    "size": a.meta.size,
                    "mime": line(&a.meta.mime, 80),
                }))
                .collect::<Vec<_>>()),
        );
    }
    if !listed.is_empty() {
        root.insert(
            "skipped".into(),
            json!(listed
                .iter()
                .map(|l| json!({
                    "name": line(&l.name, 120),
                    "size": l.size,
                    "reason": line(&l.reason, 80),
                }))
                .collect::<Vec<_>>()),
        );
    }
    let payload = serde_json::to_vec(&Value::Object(root)).unwrap_or_default();
    let ts = ctx.ts_secs.to_string();
    let mut headers = vec![
        ("X-Ymux-Timestamp".to_string(), ts.clone()),
        ("X-Ymux-Delivery".to_string(), line(ctx.delivery_id, 80)),
    ];
    if !ctx.secret.is_empty() {
        headers.push((
            "X-Ymux-Signature".to_string(),
            hmac::sign(ctx.secret, &ts, &payload),
        ));
    }
    if attached.is_empty() {
        return Built {
            content_type: "application/json".to_string(),
            headers,
            body: payload,
        };
    }
    let mut parts = vec![Part {
        name: "payload".into(),
        filename: None,
        content_type: Some("application/json".into()),
        data: payload,
    }];
    for (i, a) in attached.iter().enumerate() {
        parts.push(Part {
            name: format!("file{}", i + 1),
            filename: Some(a.meta.name.clone()),
            content_type: Some(non_empty_mime(&a.meta.mime)),
            data: a.data.clone(),
        });
    }
    let (content_type, body) = encode_multipart(ctx.boundary, &parts);
    Built {
        content_type,
        headers,
        body,
    }
}

fn non_empty_mime(m: &str) -> String {
    let m = clean(m, false);
    if m.is_empty() {
        "application/octet-stream".to_string()
    } else {
        m
    }
}

fn slack_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn build_slack(views: &[View], attached: &[Attachment], listed: &[ListedFile]) -> Built {
    let mut blocks: Vec<Value> = Vec::new();
    for v in views {
        blocks.push(json!({
            "type": "header",
            "text": {"type": "plain_text", "text": clip(&v.title, 150), "emoji": false}
        }));
        let mut text = String::new();
        for b in &v.bullets {
            text.push_str(&format!("\u{2022} {}\n", slack_escape(b)));
        }
        for (k, val) in v.facts.iter().filter(|(k, _)| {
            k == "Files" || k == "Tests" || k == "Duration" || k == "Prompt" || k == "Directory"
        }) {
            text.push_str(&format!("*{}:* {}\n", k, slack_escape(val)));
        }
        let text = clip(text.trim(), 3000);
        if !text.is_empty() {
            blocks.push(json!({"type": "section", "text": {"type": "mrkdwn", "text": text}}));
        }
        let ctx_line = v
            .facts
            .iter()
            .filter(|(k, _)| {
                matches!(
                    k.as_str(),
                    "Workspace" | "Agent" | "Branch" | "Pane" | "Account"
                )
            })
            .map(|(_, val)| slack_escape(val))
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
        if !ctx_line.is_empty() {
            blocks.push(json!({
                "type": "context",
                "elements": [{"type": "mrkdwn", "text": clip(&ctx_line, 3000)}]
            }));
        }
    }
    let files = file_lines(attached, listed, true);
    if !files.is_empty() {
        let body = files
            .iter()
            .map(|f| format!("\u{2022} {}", slack_escape(f)))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": clip(&format!("*Files*\n{body}"), 3000)}
        }));
    }
    let first = views.first().map(|v| v.title.as_str()).unwrap_or("ymux");
    let text = if views.len() > 1 {
        format!("ymux: {} updates \u{2014} {}", views.len(), first)
    } else {
        format!("ymux: {first}")
    };
    let body = json!({"text": clip(&slack_escape(&text), 300), "blocks": blocks});
    json_built(&body)
}

fn json_built(v: &Value) -> Built {
    Built {
        content_type: "application/json".to_string(),
        headers: Vec::new(),
        body: serde_json::to_vec(v).unwrap_or_default(),
    }
}

fn embed_chars(e: &Value) -> usize {
    let n = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|s| s.chars().count())
            .unwrap_or(0)
    };
    let mut t = n(e.get("title")) + n(e.get("description"));
    if let Some(fs) = e.get("fields").and_then(Value::as_array) {
        for f in fs {
            t += n(f.get("name")) + n(f.get("value"));
        }
    }
    t
}

/// Keeps the sum of embed text under Discord's 6000-char message limit.
fn fit_embeds(embeds: &mut [Value]) {
    let total = |es: &[Value]| es.iter().map(embed_chars).sum::<usize>();
    if total(embeds) <= 5800 {
        return;
    }
    for e in embeds.iter_mut() {
        if let Some(fs) = e.get_mut("fields").and_then(Value::as_array_mut) {
            fs.truncate(2);
        }
    }
    if total(embeds) <= 5800 {
        return;
    }
    for e in embeds.iter_mut() {
        if let Some(o) = e.as_object_mut() {
            o.remove("fields");
        }
    }
    for cap in [300usize, 150, 60] {
        if total(embeds) <= 5800 {
            return;
        }
        for e in embeds.iter_mut() {
            let cut = e
                .get("description")
                .and_then(Value::as_str)
                .map(|s| clip(s, cap));
            if let (Some(c), Some(o)) = (cut, e.as_object_mut()) {
                o.insert("description".into(), json!(c));
            }
            let cut = e
                .get("title")
                .and_then(Value::as_str)
                .map(|s| clip(s, cap.min(100)));
            if let (Some(c), Some(o)) = (cut, e.as_object_mut()) {
                o.insert("title".into(), json!(c));
            }
        }
    }
}

const DISCORD_MAX_FILES: usize = 10;

fn build_discord(
    views: &[View],
    ctx: &BuildCtx<'_>,
    attached: &[Attachment],
    listed: &[ListedFile],
) -> Built {
    let n_up = attached.len().min(DISCORD_MAX_FILES);
    let (uploads, overflow) = attached.split_at(n_up);
    let mut embeds: Vec<Value> = views
        .iter()
        .map(|v| {
            let desc = v
                .bullets
                .iter()
                .map(|b| format!("\u{2022} {b}"))
                .collect::<Vec<_>>()
                .join("\n");
            let fields: Vec<Value> = v
                .facts
                .iter()
                .take(25)
                .map(|(k, val)| {
                    json!({
                        "name": clip(k, 256),
                        "value": clip(val, 1024),
                        "inline": !matches!(k.as_str(), "Files" | "Prompt" | "Directory"),
                    })
                })
                .collect();
            let color: u32 = if v.failed {
                0xf85149
            } else if v.attention {
                0xd29922
            } else {
                0x3fb950
            };
            let mut m = Map::new();
            m.insert("title".into(), json!(clip(&v.title, 256)));
            m.insert("description".into(), json!(clip(&desc, 4096)));
            m.insert("color".into(), json!(color));
            m.insert("timestamp".into(), json!(iso8601(v.ts)));
            if !fields.is_empty() {
                m.insert("fields".into(), Value::Array(fields));
            }
            Value::Object(m)
        })
        .collect();
    let rest = file_lines(&[], listed, true)
        .into_iter()
        .chain(file_lines(overflow, &[], true))
        .collect::<Vec<_>>();
    if !rest.is_empty() {
        let text = rest
            .iter()
            .map(|f| format!("\u{2022} {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(last) = embeds.last_mut().and_then(Value::as_object_mut) {
            if let Some(Value::Array(fs)) = last.get_mut("fields") {
                if fs.len() < 25 {
                    fs.push(json!({"name": "Files", "value": clip(&text, 1024), "inline": false}));
                }
            } else {
                last.insert(
                    "fields".into(),
                    json!([{"name": "Files", "value": clip(&text, 1024), "inline": false}]),
                );
            }
        }
    }
    fit_embeds(&mut embeds);
    let content = if views.len() > 1 {
        format!("ymux: {} updates", views.len())
    } else {
        "ymux".to_string()
    };
    let mut root = Map::new();
    root.insert("content".into(), json!(content));
    root.insert("embeds".into(), Value::Array(embeds));
    root.insert("allowed_mentions".into(), json!({"parse": []}));
    if uploads.is_empty() {
        return json_built(&Value::Object(root));
    }
    root.insert(
        "attachments".into(),
        json!(uploads
            .iter()
            .enumerate()
            .map(|(i, a)| json!({"id": i, "filename": header_token(&a.meta.name)}))
            .collect::<Vec<_>>()),
    );
    let payload = serde_json::to_vec(&Value::Object(root)).unwrap_or_default();
    let mut parts = vec![Part {
        name: "payload_json".into(),
        filename: None,
        content_type: Some("application/json".into()),
        data: payload,
    }];
    for (i, a) in uploads.iter().enumerate() {
        parts.push(Part {
            name: format!("files[{i}]"),
            filename: Some(a.meta.name.clone()),
            content_type: Some(non_empty_mime(&a.meta.mime)),
            data: a.data.clone(),
        });
    }
    let (content_type, body) = encode_multipart(ctx.boundary, &parts);
    Built {
        content_type,
        headers: Vec::new(),
        body,
    }
}

fn build_teams(views: &[View], attached: &[Attachment], listed: &[ListedFile]) -> Built {
    let mut body: Vec<Value> = Vec::new();
    for v in views {
        body.push(json!({
            "type": "TextBlock", "text": clip(&v.title, 300),
            "weight": "Bolder", "size": "Medium", "wrap": true
        }));
        if !v.bullets.is_empty() {
            let t = v
                .bullets
                .iter()
                .map(|b| format!("- {b}"))
                .collect::<Vec<_>>()
                .join("\n");
            body.push(json!({"type": "TextBlock", "text": clip(&t, 2000), "wrap": true}));
        }
        if !v.facts.is_empty() {
            let facts: Vec<Value> = v
                .facts
                .iter()
                .map(|(k, val)| json!({"title": clip(k, 60), "value": clip(val, 600)}))
                .collect();
            body.push(json!({"type": "FactSet", "facts": facts}));
        }
    }
    let files = file_lines(attached, listed, true);
    if !files.is_empty() {
        let t = format!(
            "Files\n{}",
            files
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        body.push(
            json!({"type": "TextBlock", "text": clip(&t, 2000), "wrap": true, "isSubtle": true}),
        );
    }
    let card = json!({
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": body,
    });
    json_built(&json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": card,
        }]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str) -> DeliveryEvent {
        DeliveryEvent {
            kind: kind.into(),
            ts: 1_700_000_000_000,
            ws_name: "web".into(),
            pane_title: "claude".into(),
            agent: "claude".into(),
            account_name: "work".into(),
            branch: "main".into(),
            cwd_display: "~/repo".into(),
            headline: "Changed 2 files".into(),
            bullets: vec!["Tests passed".into()],
            files: vec![super::super::FileStat {
                path: "src/a.rs".into(),
                add: 3,
                del: 1,
            }],
            tests: Some(super::super::TestInfo {
                runner: "cargo".into(),
                passed: Some(true),
            }),
            duration_ms: Some(125_000),
            prompt: Some("secret prompt".into()),
            plan: None,
        }
    }

    fn ctx<'a>(secret: &'a str) -> BuildCtx<'a> {
        BuildCtx {
            delivery_id: "d-1",
            ts_secs: 1_700_000_000,
            secret,
            boundary: "BOUND",
        }
    }

    fn dest(format: &str) -> WebhookDestination {
        WebhookDestination {
            format: format.into(),
            ..Default::default()
        }
    }

    fn att(name: &str) -> Attachment {
        Attachment {
            meta: AttachmentRef {
                name: name.into(),
                size: 3,
                mime: "image/png".into(),
                path: "/x".into(),
                ..AttachmentRef::default()
            },
            data: vec![1, 2, 3],
        }
    }

    fn json_of(b: &Built) -> Value {
        serde_json::from_slice(&b.body).unwrap()
    }

    #[test]
    fn iso8601_known_values() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(-1000), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn clip_and_clean() {
        assert_eq!(clip("abcdef", 4), "abc\u{2026}");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip("한글한글한글", 3).chars().count(), 3);
        assert_eq!(clean("a\u{1b}[31m\u{202e}b\nc", false), "a[31mb c");
        assert_eq!(clean("a\nb", true), "a\nb");
    }

    #[test]
    fn json_summary_exact() {
        let b = build(&dest("json"), &ctx(""), &[ev("turn_end")], &[], &[]).unwrap();
        assert_eq!(b.content_type, "application/json");
        assert_eq!(
            json_of(&b),
            json!({"v":1,"source":"ymux","events":[{
                "kind":"turn_end","ts":1_700_000_000_000i64,"workspace":"web","agent":"claude",
                "branch":"main","headline":"Changed 2 files","bullets":["Tests passed"]}]})
        );
        assert_eq!(
            b.headers,
            vec![
                ("X-Ymux-Timestamp".to_string(), "1700000000".to_string()),
                ("X-Ymux-Delivery".to_string(), "d-1".to_string())
            ]
        );
    }

    #[test]
    fn json_signature_over_ts_dot_payload() {
        let b = build(&dest("json"), &ctx("s3cret"), &[ev("turn_end")], &[], &[]).unwrap();
        let sig = b
            .headers
            .iter()
            .find(|(k, _)| k == "X-Ymux-Signature")
            .unwrap()
            .1
            .clone();
        assert_eq!(sig, hmac::sign("s3cret", "1700000000", &b.body));
    }

    #[test]
    fn json_full_detail_and_prompt_gating() {
        let mut d = dest("json");
        d.detail = "full".into();
        let v = json_of(&build(&d, &ctx(""), &[ev("turn_end")], &[], &[]).unwrap());
        let e = &v["events"][0];
        assert_eq!(e["files"][0]["path"], "src/a.rs");
        assert_eq!(e["duration_ms"], 125_000);
        assert_eq!(e["tests"]["passed"], true);
        assert!(e.get("prompt").is_none());
        d.include_prompts = true;
        let v = json_of(&build(&d, &ctx(""), &[ev("turn_end")], &[], &[]).unwrap());
        assert_eq!(v["events"][0]["prompt"], "secret prompt");
    }

    #[test]
    fn summary_detail_never_leaks_prompt_even_if_enabled() {
        let mut d = dest("json");
        d.include_prompts = true;
        let b = build(&d, &ctx(""), &[ev("turn_end")], &[], &[]).unwrap();
        assert!(!String::from_utf8_lossy(&b.body).contains("secret prompt"));
    }

    #[test]
    fn event_filter_and_batch_cap() {
        let mut d = dest("json");
        d.events = vec!["attention".into()];
        assert!(build(&d, &ctx(""), &[ev("turn_end")], &[], &[]).is_none());
        d.events = vec!["turn_end".into()];
        let many: Vec<_> = (0..15).map(|_| ev("turn_end")).collect();
        let v = json_of(&build(&d, &ctx(""), &many, &[], &[]).unwrap());
        assert_eq!(v["events"].as_array().unwrap().len(), MAX_BATCH);
    }

    #[test]
    fn json_multipart_payload_first_then_files() {
        let mut d = dest("json");
        d.attach = true;
        let b = build(
            &d,
            &ctx("k"),
            &[ev("turn_end")],
            &[att("a.png"), att("b.png")],
            &[],
        )
        .unwrap();
        assert_eq!(b.content_type, "multipart/form-data; boundary=BOUND");
        let s = String::from_utf8_lossy(&b.body).to_string();
        let p = s.find("name=\"payload\"").unwrap();
        let f1 = s.find("name=\"file1\"; filename=\"a.png\"").unwrap();
        let f2 = s.find("name=\"file2\"; filename=\"b.png\"").unwrap();
        assert!(p < f1 && f1 < f2);
        assert!(s.ends_with("--BOUND--\r\n"));
        assert!(s.contains("\"part\":\"file1\""));
        assert!(b.headers.iter().any(|(k, _)| k == "X-Ymux-Signature"));
    }

    #[test]
    fn attachments_ignored_when_attach_off() {
        let b = build(
            &dest("json"),
            &ctx(""),
            &[ev("turn_end")],
            &[att("a.png")],
            &[],
        )
        .unwrap();
        assert_eq!(b.content_type, "application/json");
    }

    #[test]
    fn multipart_encoder_exact() {
        let (ct, body) = encode_multipart(
            "B",
            &[
                Part {
                    name: "a".into(),
                    filename: None,
                    content_type: Some("text/plain".into()),
                    data: b"hi".to_vec(),
                },
                Part {
                    name: "f\"x".into(),
                    filename: Some("../p\r\nath.txt".into()),
                    content_type: None,
                    data: vec![0, 255],
                },
            ],
        );
        assert_eq!(ct, "multipart/form-data; boundary=B");
        let mut want = b"--B\r\nContent-Disposition: form-data; name=\"a\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--B\r\nContent-Disposition: form-data; name=\"fx\"; filename=\"..p  ath.txt\"\r\n\r\n".to_vec();
        want.extend_from_slice(&[0, 255]);
        want.extend_from_slice(b"\r\n--B--\r\n");
        assert_eq!(body, want);
        assert_eq!(
            encode_multipart("a b\r\n", &[]).0,
            "multipart/form-data; boundary=ab"
        );
        assert_eq!(
            encode_multipart("", &[]).0,
            "multipart/form-data; boundary=ymux-boundary"
        );
    }

    #[test]
    fn slack_blocks_escape_and_limits() {
        let mut e = ev("turn_end");
        e.headline = "<!channel> & co".into();
        e.bullets = vec!["x".repeat(5000), "<@U1> hi".into()];
        let b = build(&dest("slack"), &ctx(""), &[e], &[], &[]).unwrap();
        let v = json_of(&b);
        let blocks = v["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[1]["type"], "section");
        assert_eq!(blocks[2]["type"], "context");
        let header = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(header.contains("<!channel>"));
        assert!(header.chars().count() <= 150);
        let sec = blocks[1]["text"]["text"].as_str().unwrap();
        assert!(sec.chars().count() <= 3000);
        assert!(!sec.contains("<@U1>"));
        assert!(v["text"]
            .as_str()
            .unwrap()
            .contains("&lt;!channel&gt; &amp; co"));
    }

    #[test]
    fn slack_lists_files_even_when_attach_on() {
        let mut d = dest("slack");
        d.attach = true;
        let l = ListedFile {
            name: "big.mov".into(),
            size: 5_000_000,
            reason: "over cap".into(),
        };
        let v = json_of(&build(&d, &ctx(""), &[ev("turn_end")], &[att("a.png")], &[l]).unwrap());
        let last = v["blocks"].as_array().unwrap().last().unwrap().clone();
        let t = last["text"]["text"].as_str().unwrap();
        assert!(t.contains("a.png (3 B) (not uploaded)"));
        assert!(t.contains("big.mov (4.8 MB) \u{2014} over cap"));
    }

    #[test]
    fn discord_embed_shape() {
        let mut d = dest("discord");
        d.detail = "full".into();
        let b = build(&d, &ctx(""), &[ev("turn_end")], &[], &[]).unwrap();
        let v = json_of(&b);
        assert_eq!(v["allowed_mentions"], json!({"parse": []}));
        let e = &v["embeds"][0];
        assert_eq!(e["color"], 0x3fb950);
        assert_eq!(e["timestamp"], "2023-11-14T22:13:20Z");
        assert_eq!(e["description"], "\u{2022} Tests passed");
        assert!(e["fields"].as_array().unwrap().len() <= 25);
        assert!(v.get("attachments").is_none());
    }

    #[test]
    fn discord_total_stays_under_6000() {
        let mut d = dest("discord");
        d.detail = "full".into();
        let mut e = ev("turn_end");
        e.bullets = vec!["y".repeat(300); 8];
        e.files = (0..30)
            .map(|i| super::super::FileStat {
                path: format!("{}{i}", "p".repeat(100)),
                add: 1,
                del: 1,
            })
            .collect();
        let many: Vec<_> = (0..10).map(|_| e.clone()).collect();
        let v = json_of(&build(&d, &ctx(""), &many, &[], &[]).unwrap());
        let total: usize = v["embeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(embed_chars)
            .sum();
        assert!(total <= 6000, "{total}");
        for em in v["embeds"].as_array().unwrap() {
            assert!(em["description"].as_str().unwrap().chars().count() <= 4096);
        }
    }

    #[test]
    fn discord_multipart_files() {
        let mut d = dest("discord");
        d.attach = true;
        let files: Vec<_> = (0..12).map(|i| att(&format!("f{i}.png"))).collect();
        let b = build(&d, &ctx(""), &[ev("turn_end")], &files, &[]).unwrap();
        assert_eq!(b.content_type, "multipart/form-data; boundary=BOUND");
        let s = String::from_utf8_lossy(&b.body).to_string();
        assert!(s.find("name=\"payload_json\"").unwrap() < s.find("name=\"files[0]\"").unwrap());
        assert!(s.contains("name=\"files[9]\""));
        assert!(!s.contains("name=\"files[10]\""));
        assert!(s.contains("f10.png (3 B) (not uploaded)"));
        assert!(s.contains("\"allowed_mentions\":{\"parse\":[]}"));
    }

    #[test]
    fn teams_adaptive_card_envelope() {
        let mut d = dest("teams");
        d.detail = "full".into();
        let v = json_of(&build(&d, &ctx(""), &[ev("attention")], &[], &[]).unwrap());
        assert_eq!(v["type"], "message");
        let a = &v["attachments"][0];
        assert_eq!(a["contentType"], "application/vnd.microsoft.card.adaptive");
        assert_eq!(a["content"]["type"], "AdaptiveCard");
        assert_eq!(a["content"]["version"], "1.4");
        let body = a["content"]["body"].as_array().unwrap();
        assert_eq!(body[0]["type"], "TextBlock");
        assert!(body[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Needs attention"));
        assert!(body.iter().any(|b| b["type"] == "FactSet"));
    }

    #[test]
    fn untrusted_text_is_cleaned_everywhere() {
        let mut e = ev("turn_end");
        e.headline = "a\u{1b}[2Jb\u{202e}c".into();
        e.agent = "x\r\ny".into();
        for f in ["json", "slack", "discord", "teams"] {
            let b = build(&dest(f), &ctx(""), &[e.clone()], &[], &[]).unwrap();
            let s = String::from_utf8(b.body).unwrap();
            assert!(!s.contains('\u{1b}') && !s.contains("\\u001b"), "{f}");
            assert!(!s.contains('\u{202e}') && !s.contains("\\u202e"), "{f}");
        }
    }

    #[test]
    fn empty_and_default_events_do_not_panic() {
        let mut d = dest("json");
        d.events = vec![String::new()];
        for f in ["json", "slack", "discord", "teams", "weird"] {
            d.format = f.into();
            let b = build(&d, &ctx(""), &[DeliveryEvent::default()], &[], &[]).unwrap();
            assert!(!b.body.is_empty());
        }
    }
}
