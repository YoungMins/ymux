//! Desktop glue for webhooks and LLM summaries: the managed runtime, the hub
//! listener, the attachment planner and the commands the UI uses.
//!
//! Everything pure lives in the sibling modules and `summarizer`; this file
//! only wires them to `AppState`. Created in `setup` after
//! `activity::runtime::start` (it subscribes to that runtime's hub).
//!
//! Secrets (URL, HMAC secret, API key) are write-only over IPC: no command
//! here returns a value from the store, and no error carries a URL.
//!
//! Summary timing (turn_end with a non-`rule` provider): the event is NOT
//! published until the summary job finishes, then published once with the
//! model's text as the first bullet and debounced like any other event. The
//! wait is bounded by `summarizer.timeout_secs` per job (jobs queue: one in
//! flight, `QUEUE_CAP` waiting). A full queue, an error or a timeout falls
//! back to the rule summary, which is published immediately. `attention`
//! events never wait.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Request;
use tauri::{AppHandle, Manager, State, Webview};
use uuid::Uuid;

use super::client::{self, SendOutcome};
use super::delivery::{event_from_record, DestStatus, Dispatcher, Options};
use super::payload::{self, BuildCtx};
use super::{is_valid_dest_id, url_hint, DeliveryEvent, PlanHint, WebhookDestination};
use crate::activity::runtime::ActivityRuntime;
use crate::activity::store::{kind, ActivityStore, Record};
use crate::attach::{self, Ctx};
use crate::commands::AppState;
use crate::config::model::Config;
use crate::fspath::guard_local;
use crate::secrets::SecretStore;
use crate::summarizer::rule::Summary;
use crate::summarizer::transport::{self, Executor, Job, Summarizer};
use crate::{YmuxError, YmuxResult};

/// Secrets-store id of the summarizer's API key (field `api_key`). Not a
/// destination UUID, so it can never collide with one; `sync` keeps it when
/// pruning removed destinations.
pub const SUMMARIZER_ID: &str = "summarizer";

const MIB: u64 = 1024 * 1024;

/// Managed state.
pub struct WebhookRuntime {
    pub secrets: Arc<SecretStore>,
    dests: Arc<RwLock<Vec<WebhookDestination>>>,
    dispatcher: Arc<Dispatcher>,
    exec: Executor,
    /// Kept alive: its thread ends when the last handle is dropped.
    _summarizer: Option<Arc<Summarizer>>,
}

impl WebhookRuntime {
    /// Make the dispatcher and the secrets store follow `list` (the backend
    /// config after a merge): replace the shared list, wake/stop workers, and
    /// drop the secrets of removed destinations.
    ///
    /// A no-op when the list is unchanged (every layout save lands here). Only
    /// ids that were in the previous list and are gone from the new one lose
    /// their secrets, and never when the config was defaulted at startup.
    pub fn sync_destinations(&self, list: &[WebhookDestination]) {
        let removed = {
            let mut cur = self.dests.write();
            if cur.as_slice() == list {
                return;
            }
            let removed = removed_ids(&cur, list);
            *cur = list.to_vec();
            removed
        };
        self.dispatcher.sync();
        if removed.is_empty() || !PRUNE_ALLOWED.load(Ordering::Relaxed) {
            return;
        }
        // Off the caller's thread: unsealing the store is a DPAPI/keychain call.
        let secrets = self.secrets.clone();
        std::thread::spawn(move || {
            for id in removed {
                if let Err(e) = secrets.remove_dest(&id) {
                    tracing::warn!(error = %e, "removing webhook secrets failed");
                }
            }
        });
    }
}

/// Whether removed destinations may take their secrets with them. Off until
/// `main` says the config was really read from disk: a defaulted config is
/// not evidence that the user deleted anything.
static PRUNE_ALLOWED: AtomicBool = AtomicBool::new(false);

pub fn set_prune_allowed(allowed: bool) {
    PRUNE_ALLOWED.store(allowed, Ordering::Relaxed);
}

/// Ids present in `prev` but not in `next`.
fn removed_ids(prev: &[WebhookDestination], next: &[WebhookDestination]) -> Vec<String> {
    prev.iter()
        .filter(|p| !next.iter().any(|n| n.id == p.id))
        .map(|p| p.id.clone())
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn ws_name(cfg: &Config, ws: u32) -> String {
    cfg.workspaces
        .iter()
        .find(|w| w.id == ws)
        .map(|w| w.name.clone())
        .unwrap_or_default()
}

/// Maps a stored record to a delivery event with its planner hint. The
/// workspace name and the live directory come from backend state.
fn event_for(
    app: &AppHandle,
    cfg: &Config,
    rec: &Record,
    with_prompt: bool,
) -> Option<DeliveryEvent> {
    let mut ev = event_from_record(rec, &ws_name(cfg, rec.ws), with_prompt)?;
    let live = rec
        .pane
        .parse::<Uuid>()
        .ok()
        .and_then(|p| app.state::<AppState>().pty.cwd_for(p));
    ev.plan = Some(PlanHint {
        cwd: live.unwrap_or_else(|| rec.cwd.clone()),
        turn_start_ms: rec
            .data
            .get("started_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        turn_end_ms: rec.ts,
    });
    Some(ev)
}

/// Attachment planner: the repo root is derived from the backend's cwd for the
/// pane (never a hook claim); everything else is `attach::eligible`'s call.
fn plan(
    app: &AppHandle,
    ev: &DeliveryEvent,
    dest: &WebhookDestination,
) -> (
    Vec<crate::webhooks::AttachmentRef>,
    Vec<crate::webhooks::ListedFile>,
) {
    let Some(hint) = &ev.plan else {
        return (Vec::new(), Vec::new());
    };
    let Ok(root) = crate::git::repo_root(Path::new(&hint.cwd)) else {
        return (Vec::new(), Vec::new());
    };
    let ctx = Ctx {
        root,
        report_dir: app
            .state::<AppState>()
            .config
            .snapshot()
            .activity
            .report_dir,
        turn_start_ms: hint.turn_start_ms,
        turn_end_ms: hint.turn_end_ms,
        allow_types: dest.attach_types.clone(),
        max_file_bytes: u64::from(dest.max_file_mb) * MIB,
        max_total_bytes: u64::from(dest.max_total_mb) * MIB,
        max_files: dest.max_files as usize,
    };
    let candidates: Vec<String> = ev.files.iter().map(|f| f.path.clone()).collect();
    attach::eligible(&candidates, &ctx)
}

/// Redacted facts for the model: counts, outcome and file names. No prompt, no
/// assistant text, no command lines.
fn facts_json(rec: &Record) -> String {
    let d = &rec.data;
    let files: Vec<&Value> = d
        .get("files")
        .and_then(Value::as_array)
        .map(|a| a.iter().take(30).collect())
        .unwrap_or_default();
    json!({
        "agent": rec.agent,
        "branch": rec.branch,
        "outcome": d.get("outcome"),
        "duration_ms": d.get("duration_ms"),
        "tool_count": d.get("tool_count"),
        "tools": d.get("tools"),
        "files": files,
        "test": d.get("test"),
        "permission_requests": d.get("permission_requests"),
        "stats": d.get("stats"),
        "rule_summary": d.get("summary"),
    })
    .to_string()
}

/// The hub listener. Runs on the activity worker thread: only cheap work and
/// non-blocking hand-offs happen here.
fn on_record(
    app: &AppHandle,
    dests: &RwLock<Vec<WebhookDestination>>,
    dispatcher: &Arc<Dispatcher>,
    summarizer: Option<&Arc<Summarizer>>,
    rec: &Record,
) {
    let want_kind = match rec.kind.as_str() {
        kind::TURN_END => "turn_end",
        kind::PERMISSION => "attention",
        _ => return,
    };
    let (any, with_prompt) = {
        let l = dests.read();
        let wants = l
            .iter()
            .filter(|d| d.enabled && d.events.iter().any(|k| k == want_kind));
        let (mut any, mut prompt) = (false, false);
        for d in wants {
            any = true;
            prompt |= d.include_prompts && d.detail == "full";
        }
        (any, prompt)
    };
    if !any {
        return;
    }
    let cfg = app.state::<AppState>().config.snapshot();
    let Some(ev) = event_for(app, &cfg, rec, with_prompt) else {
        return;
    };
    let llm = want_kind == "turn_end" && cfg.summarizer.provider != "rule";
    let Some(s) = summarizer.filter(|_| llm) else {
        dispatcher.publish(&ev);
        return;
    };
    let fallback = Summary {
        headline: ev.headline.clone(),
        bullets: ev.bullets.clone(),
    };
    // The job owns the event; if the queue is full the job is dropped unrun
    // and the event is taken back from here.
    let held = Arc::new(parking_lot::Mutex::new(Some(ev)));
    let (d2, h2) = (dispatcher.clone(), held.clone());
    let submitted = s.submit(Job {
        facts_json: facts_json(rec),
        fallback,
        lang: cfg.activity.language.clone(),
        done: Box::new(move |res| {
            if let Some(mut e) = h2.lock().take() {
                if res.used_llm {
                    e.headline = res.summary.headline;
                    e.bullets = res.summary.bullets.into_iter().take(10).collect();
                }
                d2.publish(&e);
            }
        }),
    });
    if !submitted {
        // Queue full: keep the rule summary and do not wait.
        if let Some(e) = held.lock().take() {
            dispatcher.publish(&e);
        }
    }
}

/// Create the runtime, start the dispatcher and the summarizer, subscribe to
/// the activity hub and manage the state. Call once in `setup`, after
/// `activity::runtime::start`.
pub fn start(app: &AppHandle) {
    let dir = ytheme::config_dir().unwrap_or_else(|| std::path::PathBuf::from("./ymux-config"));
    let secrets = Arc::new(SecretStore::new(&dir));
    let cfg = app.state::<AppState>().config.snapshot();
    let dests = Arc::new(RwLock::new(cfg.webhooks.clone()));
    let planner_app = app.clone();
    let opts = Options {
        planner: Arc::new(move |ev, dest| plan(&planner_app, ev, dest)),
        ..Options::default()
    };
    let dispatcher = Arc::new(Dispatcher::start_with(
        &dir.join("webhooks"),
        secrets.clone(),
        dests.clone(),
        client::send,
        opts,
    ));
    let exec = transport::http_executor();
    let settings_app = app.clone();
    let settings_secrets = secrets.clone();
    let summarizer = Summarizer::start(
        exec.clone(),
        Arc::new(move || {
            (
                settings_app
                    .state::<AppState>()
                    .config
                    .snapshot()
                    .summarizer,
                settings_secrets
                    .get(SUMMARIZER_ID, "api_key")
                    .unwrap_or_default(),
            )
        }),
    )
    .map(Arc::new);

    if let Some(act) = app.try_state::<ActivityRuntime>() {
        let (a, d, disp, s) = (
            app.clone(),
            dests.clone(),
            dispatcher.clone(),
            summarizer.clone(),
        );
        act.hub.subscribe(Arc::new(move |rec| {
            on_record(&a, &d, &disp, s.as_ref(), rec);
        }));
    } else {
        tracing::warn!("activity runtime missing: webhooks will not receive events");
    }
    app.manage(WebhookRuntime {
        secrets,
        dests,
        dispatcher,
        exec,
        _summarizer: summarizer,
    });
}

// ---------------------------------------------------------------- commands

fn destination(rt: &WebhookRuntime, id: &str) -> YmuxResult<WebhookDestination> {
    if !is_valid_dest_id(id) {
        return Err(YmuxError::Config("invalid destination id".into()));
    }
    rt.dests
        .read()
        .iter()
        .find(|d| d.id == id)
        .cloned()
        .ok_or_else(|| YmuxError::Config("unknown destination".into()))
}

/// Store (or, with an empty `value`, clear) a destination's `url` or `secret`.
/// Write-only. The destination must already be in the saved config. Returns
/// the new display hint (`scheme://host`) so the UI can show it without ever
/// seeing the URL again.
#[tauri::command(async)]
pub fn webhook_set_secret(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
    state: State<'_, AppState>,
    id: String,
    field: String,
    value: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "webhook_set_secret")?;
    destination(&rt, &id)?;
    if field != "url" && field != "secret" {
        return Err(YmuxError::Config("unknown secret field".into()));
    }
    let value = value.trim().to_string();
    if field == "url" && !value.is_empty() && client::check_url(&value).is_err() {
        return Err(YmuxError::Config(
            "the URL must be http:// or https://".into(),
        ));
    }
    rt.secrets.set(&id, &field, &value)?;
    if field != "url" {
        return Ok(String::new());
    }
    let hint = url_hint(&value);
    state.config.update(|c| {
        if let Some(d) = c.webhooks.iter_mut().find(|d| d.id == id) {
            d.url_hint = hint.clone();
        }
    });
    state.config.flush()?;
    *rt.dests.write() = state.config.snapshot().webhooks;
    Ok(hint)
}

#[derive(Debug, Serialize)]
pub struct WebhookSecretStatus {
    pub url: bool,
    pub secret: bool,
    /// `http://` to a non-loopback host: the payload would cross the network
    /// unencrypted.
    pub insecure: bool,
}

#[tauri::command(async)]
pub fn webhook_secret_status(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
    id: String,
) -> YmuxResult<WebhookSecretStatus> {
    guard_local(&webview, &request, "webhook_secret_status")?;
    if !is_valid_dest_id(&id) {
        return Err(YmuxError::Config("invalid destination id".into()));
    }
    let s = rt.secrets.status(&id);
    let insecure = rt
        .secrets
        .get(&id, "url")
        .is_some_and(|u| client::warn_insecure(&u));
    Ok(WebhookSecretStatus {
        url: s.url,
        secret: s.secret,
        insecure,
    })
}

#[derive(Debug, Serialize)]
pub struct WebhookTestResult {
    pub ok: bool,
    pub status: Option<u16>,
    /// Kind only (`http_500`, `connect`, `no_url`, ...); never the URL.
    pub error: Option<String>,
}

/// Sends one synthetic event through the real build + send path (not the
/// queue), whatever the destination's event filter says.
#[tauri::command(async)]
pub async fn webhook_test(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
    id: String,
) -> YmuxResult<WebhookTestResult> {
    guard_local(&webview, &request, "webhook_test")?;
    let mut dest = destination(&rt, &id)?;
    let Some(url) = rt.secrets.get(&id, "url").filter(|u| !u.trim().is_empty()) else {
        return Ok(WebhookTestResult {
            ok: false,
            status: None,
            error: Some("no_url".into()),
        });
    };
    let secret = rt.secrets.get(&id, "secret").unwrap_or_default();
    dest.events = vec!["turn_end".into()];
    dest.attach = false;
    let ev = DeliveryEvent {
        kind: "turn_end".into(),
        ts: now_ms(),
        ws_name: "ymux".into(),
        agent: "ymux".into(),
        headline: "ymux test notification".into(),
        bullets: vec!["If you can read this, the webhook works.".into()],
        ..DeliveryEvent::default()
    };
    let out = tauri::async_runtime::spawn_blocking(move || {
        let delivery_id = Uuid::new_v4().simple().to_string();
        let boundary = format!("ymux-{delivery_id}");
        let ctx = BuildCtx {
            delivery_id: &delivery_id,
            ts_secs: now_ms() / 1000,
            secret: &secret,
            boundary: &boundary,
        };
        let built = payload::build(&dest, &ctx, &[ev], &[], &[])?;
        Some(client::send(&url, &built))
    })
    .await
    .map_err(|e| YmuxError::Other(format!("webhook_test: {e}")))?;
    let out: SendOutcome = out.unwrap_or_else(|| SendOutcome::failed(client::ErrorKind::Build));
    let ok = out.status.is_some_and(|s| (200..300).contains(&s));
    let error = match (out.status, out.error_kind) {
        _ if ok => None,
        (Some(s), _) => Some(format!("http_{s}")),
        (None, Some(k)) => Some(k.as_str().to_string()),
        (None, None) => Some("no_response".into()),
    };
    Ok(WebhookTestResult {
        ok,
        status: out.status,
        error,
    })
}

#[derive(Debug, Serialize)]
pub struct PreviewFile {
    pub name: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewListed {
    pub name: String,
    pub size: u64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct WebhookPreview {
    pub format: String,
    /// The JSON body that would be sent (a string when it is not JSON). The
    /// URL, secret and signature headers are not part of it.
    pub payload: Value,
    pub attachments: Vec<PreviewFile>,
    pub listed: Vec<PreviewListed>,
}

/// What the latest `turn_end` record would send to this destination.
#[tauri::command(async)]
pub fn webhook_preview(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    rt: State<'_, WebhookRuntime>,
    act: State<'_, ActivityRuntime>,
    id: String,
) -> YmuxResult<Option<WebhookPreview>> {
    guard_local(&webview, &request, "webhook_preview")?;
    let dest = destination(&rt, &id)?;
    let cfg = app.state::<AppState>().config.snapshot();
    let store = ActivityStore::new(
        act.dir.clone(),
        cfg.activity.retention_days,
        cfg.activity.max_total_mb,
    );
    let Some(rec) = store
        .read_recent(200)
        .into_iter()
        .rev()
        .find(|r| r.kind == kind::TURN_END)
    else {
        return Ok(None);
    };
    let with_prompt = dest.include_prompts && dest.detail == "full";
    let Some(mut ev) = event_for(&app, &cfg, &rec, with_prompt) else {
        return Ok(None);
    };
    if !with_prompt {
        ev.prompt = None;
    }
    // The preview shows a turn_end whatever the destination's filter says.
    let mut shown = dest.clone();
    if !shown.events.iter().any(|k| k == "turn_end") {
        shown.events.push("turn_end".into());
    }
    let (refs, mut listed) = if dest.attach {
        plan(&app, &ev, &dest)
    } else {
        (Vec::new(), Vec::new())
    };
    let ctx = BuildCtx {
        delivery_id: "preview",
        ts_secs: now_ms() / 1000,
        secret: "",
        boundary: "preview",
    };
    // Attachment bytes are not read for a preview; they are listed by name.
    let Some(built) = payload::build(&shown, &ctx, &[ev], &[], &listed.clone()) else {
        return Ok(None);
    };
    let payload = serde_json::from_slice::<Value>(&built.body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&built.body).into_owned()));
    let attachments = refs
        .into_iter()
        .map(|r| PreviewFile {
            name: r.name,
            size: r.size,
            mime: r.mime,
        })
        .collect();
    let listed = listed
        .drain(..)
        .map(|l| PreviewListed {
            name: l.name,
            size: l.size,
            reason: l.reason,
        })
        .collect();
    Ok(Some(WebhookPreview {
        format: dest.format,
        payload,
        attachments,
        listed,
    }))
}

/// Per-destination delivery state: kinds and counts only.
#[tauri::command(async)]
pub fn webhook_status(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
) -> YmuxResult<HashMap<String, DestStatus>> {
    guard_local(&webview, &request, "webhook_status")?;
    let ids: Vec<String> = rt.dests.read().iter().map(|d| d.id.clone()).collect();
    Ok(ids
        .into_iter()
        .map(|id| {
            let s = rt.dispatcher.status(&id);
            (id, s)
        })
        .collect())
}

fn transport_err(what: &str, e: transport::TransportError) -> YmuxError {
    YmuxError::Other(format!("{what}: {e:?}"))
}

/// Models the saved provider offers at the saved endpoint (the frontend
/// supplies neither). The API key comes from the store; a non-loopback
/// endpoint is refused unless `allow_remote` is saved.
#[tauri::command(async)]
pub async fn summarizer_list_models(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    rt: State<'_, WebhookRuntime>,
) -> YmuxResult<Vec<String>> {
    guard_local(&webview, &request, "summarizer_list_models")?;
    let cfg = app.state::<AppState>().config.snapshot().summarizer;
    let key = rt.secrets.get(SUMMARIZER_ID, "api_key").unwrap_or_default();
    let exec = rt.exec.clone();
    let timeout = Duration::from_secs(u64::from(cfg.timeout_secs.clamp(1, 300)));
    tauri::async_runtime::spawn_blocking(move || {
        transport::list_models(
            &exec,
            &cfg.provider,
            &cfg.endpoint,
            &key,
            cfg.allow_remote,
            timeout,
        )
    })
    .await
    .map_err(|e| YmuxError::Other(format!("summarizer_list_models: {e}")))?
    .map_err(|e| transport_err("summarizer_list_models", e))
}

#[derive(Debug, Serialize)]
pub struct SummarizerTest {
    /// False when the model could not be used and the rule text is shown.
    pub used_llm: bool,
    pub text: String,
}

/// Summarizes a fixed sample with the saved settings.
#[tauri::command(async)]
pub async fn summarizer_test(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    rt: State<'_, WebhookRuntime>,
) -> YmuxResult<SummarizerTest> {
    guard_local(&webview, &request, "summarizer_test")?;
    let cfg = app.state::<AppState>().config.snapshot();
    let key = rt.secrets.get(SUMMARIZER_ID, "api_key").unwrap_or_default();
    let exec = rt.exec.clone();
    let lang = cfg.activity.language.clone();
    let fallback = Summary {
        headline: "Changed 3 files (+42/-7)".into(),
        bullets: vec!["tests passed".into()],
    };
    let facts = json!({
        "agent": "claude", "outcome": "Completed", "duration_ms": 95000,
        "files": ["src/app.ts", "src/app.test.ts", "README.md"],
        "test": {"runner": "vitest", "passed": true},
        "rule_summary": {"headline": fallback.headline, "bullets": fallback.bullets},
    })
    .to_string();
    let res = tauri::async_runtime::spawn_blocking(move || {
        transport::summarize(&exec, &cfg.summarizer, &key, &lang, &facts, &fallback)
    })
    .await
    .map_err(|e| YmuxError::Other(format!("summarizer_test: {e}")))?;
    let mut text = res.summary.headline;
    for b in res.summary.bullets {
        text.push('\n');
        text.push_str(&b);
    }
    Ok(SummarizerTest {
        used_llm: res.used_llm,
        text,
    })
}

/// Store (or, empty, clear) the LLM API key. Write-only.
#[tauri::command(async)]
pub fn summarizer_set_api_key(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
    value: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "summarizer_set_api_key")?;
    rt.secrets.set(SUMMARIZER_ID, "api_key", value.trim())
}

/// Whether an LLM API key is stored. Never returns the key itself.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct KeyStatus {
    pub set: bool,
}

#[tauri::command(async)]
pub fn summarizer_key_status(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, WebhookRuntime>,
) -> YmuxResult<KeyStatus> {
    guard_local(&webview, &request, "summarizer_key_status")?;
    Ok(KeyStatus {
        set: rt
            .secrets
            .get(SUMMARIZER_ID, "api_key")
            .is_some_and(|k| !k.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &str) -> WebhookDestination {
        WebhookDestination {
            id: id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn removed_ids_are_only_those_that_left_the_list() {
        let prev = [d("a"), d("b")];
        assert_eq!(removed_ids(&prev, &[d("b"), d("c")]), vec!["a"]);
        assert!(removed_ids(&prev, &prev).is_empty());
        // Nothing was known before, so nothing can be "removed".
        assert!(removed_ids(&[], &[d("x")]).is_empty());
    }
}
