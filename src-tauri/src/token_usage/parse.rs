use super::model::{Event, Quota, TokenTotals};
use crate::agent_scan_disk::parse_rfc3339_secs;
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    total_tokens: Option<u64>,
}
impl Usage {
    fn totals(&self, codex: bool) -> TokenTotals {
        let read = if codex {
            self.cached_input_tokens.min(self.input_tokens)
        } else {
            self.cache_read_input_tokens
        };
        let input = if codex {
            self.input_tokens.saturating_sub(read)
        } else {
            self.input_tokens
        };
        let total = self.total_tokens.unwrap_or_else(|| {
            input
                .saturating_add(self.output_tokens)
                .saturating_add(read)
                .saturating_add(self.cache_creation_input_tokens)
        });
        TokenTotals {
            input,
            output: self.output_tokens,
            cache_read: read,
            cache_write: self.cache_creation_input_tokens,
            total,
        }
    }
}
#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    uuid: Option<String>,
    message: Option<Message>,
    payload: Option<Payload>,
}
#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    info: Option<Info>,
    rate_limits: Option<Limits>,
}
#[derive(Deserialize)]
struct Info {
    total_token_usage: Option<Usage>,
    model: Option<String>,
}
#[derive(Deserialize)]
struct Limits {
    primary: Option<Limit>,
    secondary: Option<Limit>,
}
#[derive(Deserialize)]
struct Limit {
    used_percent: f64,
    window_minutes: Option<u64>,
    resets_at: Option<u64>,
}

#[derive(Default)]
pub(super) struct Parsed {
    pub events: Vec<Event>,
    pub quotas: Vec<Quota>,
    pub skipped: usize,
    session: String,
    project: String,
    model: String,
}
impl Parsed {
    pub fn new(fallback: &str) -> Self {
        Self {
            session: fallback.to_owned(),
            project: "unknown".into(),
            model: "unknown".into(),
            ..Self::default()
        }
    }
    pub fn line(&mut self, bytes: &[u8], provider: &'static str) {
        if self.events.len() >= 10_000 || self.quotas.len() >= 10_000 {
            self.skipped += 1;
            return;
        }
        let line: Line = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(_) => {
                self.skipped += 1;
                return;
            }
        };
        let too_long =
            |value: &Option<String>, limit| value.as_ref().is_some_and(|s| s.len() > limit);
        if too_long(&line.cwd, 4096)
            || too_long(&line.session_id, 256)
            || too_long(&line.uuid, 256)
            || line
                .message
                .as_ref()
                .is_some_and(|m| too_long(&m.id, 256) || too_long(&m.model, 256))
            || line.payload.as_ref().is_some_and(|p| {
                too_long(&p.cwd, 4096)
                    || too_long(&p.id, 256)
                    || too_long(&p.session_id, 256)
                    || too_long(&p.model, 256)
                    || p.info
                        .as_ref()
                        .is_some_and(|info| too_long(&info.model, 256))
            })
        {
            self.skipped += 1;
            return;
        }
        if let Some(cwd) = line.cwd {
            self.project = cwd;
        }
        let timestamp = line.timestamp.as_deref().and_then(parse_rfc3339_secs);
        match provider {
            "claude" => {
                if let Some(session) = line.session_id {
                    self.session = session;
                }
                if line.kind.as_deref() != Some("assistant") {
                    return;
                }
                let Some(message) = line.message else {
                    return;
                };
                let Some(usage) = message.usage else {
                    return;
                };
                let Some(timestamp) = timestamp else {
                    self.skipped += 1;
                    return;
                };
                let Some(id) = message.id.or(line.uuid) else {
                    self.skipped += 1;
                    return;
                };
                self.events.push(Event {
                    provider,
                    session: self.session.clone(),
                    id,
                    timestamp,
                    project: self.project.clone(),
                    model: message.model.unwrap_or_else(|| "unknown".into()),
                    tokens: usage.totals(false),
                });
            }
            "codex" => {
                let Some(payload) = line.payload else {
                    return;
                };
                match line.kind.as_deref() {
                    Some("session_meta") => {
                        if let Some(id) = payload.id.or(payload.session_id) {
                            self.session = id;
                        }
                        if let Some(cwd) = payload.cwd {
                            self.project = cwd;
                        }
                    }
                    Some("turn_context") => {
                        if let Some(model) = payload.model {
                            self.model = model;
                        }
                        if let Some(cwd) = payload.cwd {
                            self.project = cwd;
                        }
                    }
                    Some("event_msg") if payload.kind.as_deref() == Some("token_count") => {
                        let Some(timestamp) = timestamp else {
                            self.skipped += 1;
                            return;
                        };
                        if let Some(limits) = payload.rate_limits {
                            let quotas: Vec<_> = [limits.primary, limits.secondary]
                                .into_iter()
                                .flatten()
                                .filter(|q| q.used_percent.is_finite() && q.used_percent >= 0.0)
                                .filter_map(|q| {
                                    q.window_minutes.map(|window_minutes| Quota {
                                        window_minutes,
                                        used_percent: q.used_percent,
                                        resets_at: q.resets_at,
                                        observed_at: timestamp,
                                    })
                                })
                                .collect();
                            self.quotas.extend(quotas);
                        }
                        let Some(info) = payload.info else {
                            return;
                        };
                        if let Some(model) = info.model {
                            self.model = model;
                        }
                        let Some(usage) = info.total_token_usage else {
                            return;
                        };
                        let totals = usage.totals(true);
                        self.events.push(Event {
                            provider,
                            session: self.session.clone(),
                            id: totals.total.to_string(),
                            timestamp,
                            project: self.project.clone(),
                            model: self.model.clone(),
                            tokens: totals,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
