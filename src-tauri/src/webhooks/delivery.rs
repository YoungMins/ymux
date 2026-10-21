//! Webhook delivery: a `Dispatcher` with one worker thread per destination.
//!
//! Flow per destination: `publish` try_sends a `DeliveryEvent` into a bounded
//! channel (never blocks the publisher; overflow is counted) -> the worker
//! debounces and batches (<= `payload::MAX_BATCH` per message) -> each message
//! is pushed to the persisted `Queue` BEFORE it is sent -> sent through the
//! injected sender -> acked on 2xx, rescheduled with backoff on a retryable
//! failure (honouring `Retry-After`), dropped on a permanent one or after
//! `MAX_ATTEMPTS`. Workers resume queued items at startup. Destinations are
//! re-read on every pass, so add/remove/disable/enable take effect without a
//! restart (call `sync` after changing the list; `publish` also syncs).
//!
//! Attachments: the planner (`attach::eligible` in the app) is asked once
//! when a message is queued and returns each file with its identity, size and
//! mtime. Right before sending, `attach::open_verified` opens the file again
//! without following links and reads the bytes from that verified handle. A
//! file that was swapped, changed, vanished, became a link or gained a hard
//! link in between is listed instead of attached.
//!
//! Time, jitter and the HTTP sender are injected, so everything is tested
//! without a network or sleeping. No function here panics; a panicking
//! sender is treated as a retryable failure.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backoff::{classify, next_delay, Outcome};
use super::client::SendOutcome;
use super::payload::{self, Attachment, BuildCtx, Built, MAX_BATCH};
use super::queue::{Queue, QueueItem};
use super::{AttachmentRef, DeliveryEvent, FileStat, ListedFile, TestInfo, WebhookDestination};
use crate::activity::store::{kind, Record};
use crate::attach;
use crate::secrets::SecretStore;

/// Per-destination event channel capacity.
const CHANNEL_CAP: usize = 256;

pub type Sender = Arc<dyn Fn(&str, &Built) -> SendOutcome + Send + Sync>;
/// Eligibility for one event: files to attach and files only to list.
pub type Planner = Arc<
    dyn Fn(&DeliveryEvent, &WebhookDestination) -> (Vec<AttachmentRef>, Vec<ListedFile>)
        + Send
        + Sync,
>;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

/// Injection points. `Options::default()` is the production setup.
#[derive(Clone)]
pub struct Options {
    pub clock: Arc<dyn Clock>,
    /// How often an idle worker re-checks its debounce, queue and config.
    pub tick: Duration,
    /// Returns [0, 1].
    pub jitter: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub planner: Planner,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            tick: Duration::from_millis(500),
            jitter: Arc::new(|| {
                let b = uuid::Uuid::new_v4().as_bytes().to_owned();
                f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])) / f64::from(u32::MAX)
            }),
            planner: Arc::new(|_, _| (Vec::new(), Vec::new())),
        }
    }
}

/// What the UI may show about a destination. Kinds only, never a URL.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DestStatus {
    pub queued: usize,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    /// Events dropped because the worker's channel was full.
    pub dropped: u64,
}

// ------------------------------------------------------------------ stored

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredFile {
    /// Carries the approved file's identity, size and mtime.
    meta: AttachmentRef,
}

/// Payload of one queue item (one message). File bytes are NOT stored: they
/// are read at send time from a handle `attach::open_verified` has checked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    events: Vec<DeliveryEvent>,
    files: Vec<StoredFile>,
    listed: Vec<ListedFile>,
}

fn listed(name: &str, size: u64, reason: &str) -> ListedFile {
    ListedFile {
        name: name.to_string(),
        size,
        reason: reason.to_string(),
    }
}

/// A planned file that is already different from what eligibility saw (or is
/// no longer an acceptable file) is listed.
fn record_file(r: AttachmentRef) -> Result<StoredFile, ListedFile> {
    match attach::verify(&r) {
        Ok(()) => Ok(StoredFile { meta: r }),
        Err(why) => Err(listed(&r.name, r.size, why)),
    }
}

/// Reads each recorded file just before sending, from a handle that
/// `attach::open_verified` opened without following links and matched against
/// the recorded identity, size and mtime. Anything that no longer matches or
/// exceeds the destination's caps moves to the listed set.
fn load_attachments(
    stored: &Stored,
    dest: &WebhookDestination,
) -> (Vec<Attachment>, Vec<ListedFile>) {
    let mut out = Vec::new();
    let mut lst = stored.listed.clone();
    let max_files = dest.max_files as usize;
    let max_total = u64::from(dest.max_total_mb).saturating_mul(1024 * 1024);
    let max_file = u64::from(dest.max_file_mb).saturating_mul(1024 * 1024);
    let mut total = 0u64;
    for f in &stored.files {
        let m = &f.meta;
        if out.len() >= max_files || total.saturating_add(m.size) > max_total || m.size > max_file {
            lst.push(listed(&m.name, m.size, "over the limit"));
            continue;
        }
        match attach::open_verified(m) {
            Ok(data) => {
                total += m.size;
                out.push(Attachment {
                    meta: m.clone(),
                    data,
                });
            }
            Err(why) => lst.push(listed(&m.name, m.size, why)),
        }
    }
    (out, lst)
}

// ---------------------------------------------------------------- mapping

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Last two path components, for a short display form.
fn cwd_display(cwd: &str) -> String {
    let parts: Vec<&str> = cwd.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let from = parts.len().saturating_sub(2);
    parts[from..].join("/")
}

/// Maps an activity record to a delivery event: `turn_end` and `permission`
/// (-> `attention`) only. `ws_name` comes from backend state. The prompt is
/// carried only with `with_prompt` (the caller's "some destination wants
/// prompts"); each destination still gates it again.
pub fn event_from_record(rec: &Record, ws_name: &str, with_prompt: bool) -> Option<DeliveryEvent> {
    let d = &rec.data;
    let mut ev = DeliveryEvent {
        ts: i64::try_from(rec.ts).unwrap_or(i64::MAX),
        ws_name: ws_name.to_string(),
        pane_title: str_of(d, "title").unwrap_or_default().to_string(),
        agent: rec.agent.clone(),
        account_name: rec.account.clone(),
        branch: rec.branch.clone(),
        cwd_display: cwd_display(&rec.cwd),
        ..DeliveryEvent::default()
    };
    match rec.kind.as_str() {
        kind::TURN_END => {
            ev.kind = "turn_end".to_string();
            let sum = d.get("summary");
            ev.headline = sum
                .and_then(|s| str_of(s, "headline"))
                .unwrap_or("Turn finished")
                .to_string();
            ev.bullets = sum
                .and_then(|s| s.get("bullets"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .take(10)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            ev.files = d
                .get("files")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .take(50)
                        .map(|p| FileStat {
                            path: p.to_string(),
                            add: 0,
                            del: 0,
                        })
                        .collect()
                })
                .unwrap_or_default();
            ev.tests = d.get("test").filter(|t| t.is_object()).map(|t| TestInfo {
                runner: str_of(t, "runner").unwrap_or_default().to_string(),
                passed: t.get("passed").and_then(Value::as_bool),
            });
            ev.duration_ms = d.get("duration_ms").and_then(Value::as_u64);
            if with_prompt {
                ev.prompt = str_of(d, "prompt").map(str::to_string);
            }
        }
        kind::PERMISSION => {
            ev.kind = "attention".to_string();
            let tool = str_of(d, "tool").unwrap_or_default();
            ev.headline = if tool.is_empty() {
                "Permission requested".to_string()
            } else {
                format!("Permission requested: {tool}")
            };
            if let Some(c) = str_of(d, "command").filter(|c| !c.is_empty()) {
                ev.bullets = vec![c.chars().take(200).collect()];
            }
        }
        _ => return None,
    }
    Some(ev)
}

// ------------------------------------------------------------- dispatcher

enum Msg {
    /// The event and the clock time it was published at (the debounce window
    /// starts there, not when the worker thread happens to dequeue it).
    Event(Box<DeliveryEvent>, i64),
    Stop,
}

struct WorkerHandle {
    tx: SyncSender<Msg>,
    alive: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

struct Inner {
    dir: PathBuf,
    secrets: Arc<SecretStore>,
    destinations: Arc<RwLock<Vec<WebhookDestination>>>,
    sender: Sender,
    opts: Options,
    queues: Mutex<HashMap<String, Arc<Queue>>>,
    statuses: Mutex<HashMap<String, Arc<Mutex<DestStatus>>>>,
    workers: Mutex<HashMap<String, WorkerHandle>>,
    dropped: AtomicU64,
}

pub struct Dispatcher {
    inner: Arc<Inner>,
}

impl Dispatcher {
    /// Starts delivery with production options and resumes queued items of
    /// enabled destinations. `dir` holds the per-destination queue files.
    pub fn start(
        dir: &Path,
        secrets: Arc<SecretStore>,
        destinations: Arc<RwLock<Vec<WebhookDestination>>>,
        sender: impl Fn(&str, &Built) -> SendOutcome + Send + Sync + 'static,
    ) -> Dispatcher {
        Self::start_with(dir, secrets, destinations, sender, Options::default())
    }

    pub fn start_with(
        dir: &Path,
        secrets: Arc<SecretStore>,
        destinations: Arc<RwLock<Vec<WebhookDestination>>>,
        sender: impl Fn(&str, &Built) -> SendOutcome + Send + Sync + 'static,
        opts: Options,
    ) -> Dispatcher {
        let d = Dispatcher {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                secrets,
                destinations,
                sender: Arc::new(sender),
                opts,
                queues: Mutex::new(HashMap::new()),
                statuses: Mutex::new(HashMap::new()),
                workers: Mutex::new(HashMap::new()),
                dropped: AtomicU64::new(0),
            }),
        };
        d.sync();
        d
    }

    /// Reconciles workers with the destination list: spawns one per enabled
    /// destination, asks workers of disabled/removed ones to stop, and purges
    /// the queue of a removed destination. Call after the list changes.
    pub fn sync(&self) {
        let inner = &self.inner;
        let dests = inner.destinations.read().clone();
        let mut workers = inner.workers.lock();
        workers.retain(|_, w| w.alive.load(Ordering::SeqCst));
        for (id, w) in workers.iter() {
            if !dests.iter().any(|d| d.enabled && &d.id == id) {
                let _ = w.tx.try_send(Msg::Stop);
            }
        }
        for d in dests.iter().filter(|d| d.enabled && d.has_valid_id()) {
            if !workers.contains_key(&d.id) {
                if let Some(w) = Inner::spawn(inner, &d.id) {
                    workers.insert(d.id.clone(), w);
                }
            }
        }
        // A destination that no longer exists must not leave a queue behind
        // (its secrets are pruned by the owner of the list).
        let known: Vec<String> = inner.queues.lock().keys().cloned().collect();
        for id in known {
            if !dests.iter().any(|d| d.id == id) && !workers.contains_key(&id) {
                if let Some(q) = inner.queue(&id) {
                    for it in q.list() {
                        let _ = q.ack(&it.id);
                    }
                }
                inner.queues.lock().remove(&id);
                inner.statuses.lock().remove(&id);
            }
        }
    }

    /// Offers an event to every enabled destination that wants its kind. Never
    /// blocks: a full channel counts a drop.
    pub fn publish(&self, ev: &DeliveryEvent) {
        self.sync();
        let at = self.inner.opts.clock.now_ms();
        let dests = self.inner.destinations.read().clone();
        let workers = self.inner.workers.lock();
        for d in dests
            .iter()
            .filter(|d| d.enabled && d.events.iter().any(|k| k == &ev.kind))
        {
            let Some(w) = workers.get(&d.id) else {
                continue;
            };
            let mut e = ev.clone();
            if !(d.include_prompts && d.detail == "full") {
                e.prompt = None;
            }
            match w.tx.try_send(Msg::Event(Box::new(e), at)) {
                Ok(()) => {}
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                    self.inner.dropped.fetch_add(1, Ordering::SeqCst);
                    self.inner.status_of(&d.id).lock().dropped += 1;
                }
            }
        }
    }

    pub fn status(&self, id: &str) -> DestStatus {
        let mut s = self.inner.status_of(id).lock().clone();
        s.queued = self.inner.queue(id).map_or(0, |q| q.len());
        s
    }

    /// Events dropped across all destinations because a worker was behind.
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::SeqCst)
    }

    /// Asks every worker to stop (pending events are queued first) and waits.
    pub fn shutdown(&self) {
        let mut handles = Vec::new();
        {
            let mut workers = self.inner.workers.lock();
            for (_, mut w) in workers.drain() {
                let _ = w.tx.try_send(Msg::Stop);
                // A full channel: the worker also exits on its own once it
                // sees Disconnected below.
                if let Some(j) = w.join.take() {
                    handles.push((w.tx, j));
                }
            }
        }
        for (tx, j) in handles {
            drop(tx);
            let _ = j.join();
        }
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        for w in self.inner.workers.lock().values() {
            let _ = w.tx.try_send(Msg::Stop);
        }
    }
}

impl Inner {
    /// `None` for an id that is not a destination UUID.
    fn queue(&self, id: &str) -> Option<Arc<Queue>> {
        let mut queues = self.queues.lock();
        if let Some(q) = queues.get(id) {
            return Some(q.clone());
        }
        let q = Arc::new(Queue::open(&self.dir, id)?);
        queues.insert(id.to_string(), q.clone());
        Some(q)
    }

    fn status_of(&self, id: &str) -> Arc<Mutex<DestStatus>> {
        self.statuses
            .lock()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    fn dest(&self, id: &str) -> Option<WebhookDestination> {
        self.destinations
            .read()
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }

    fn spawn(this: &Arc<Inner>, id: &str) -> Option<WorkerHandle> {
        let (tx, rx) = sync_channel::<Msg>(CHANNEL_CAP);
        let alive = Arc::new(AtomicBool::new(true));
        let inner = this.clone();
        let wid = id.to_string();
        let a = alive.clone();
        let join = std::thread::Builder::new()
            .name(format!(
                "webhook-{}",
                id.chars().take(16).collect::<String>()
            ))
            .spawn(move || {
                inner.run(&wid, &rx);
                a.store(false, Ordering::SeqCst);
            })
            .ok()?;
        Some(WorkerHandle {
            tx,
            alive,
            join: Some(join),
        })
    }

    fn run(&self, id: &str, rx: &Receiver<Msg>) {
        let Some(queue) = self.queue(id) else {
            return;
        };
        let status = self.status_of(id);
        let mut pending: Vec<DeliveryEvent> = Vec::new();
        let mut first_ms = 0i64;
        let mut wait = Duration::ZERO;
        let mut queue_stuck = false;
        while let Some(cfg) = self.dest(id).filter(|d| d.enabled) {
            let mut stop = false;
            match rx.recv_timeout(wait) {
                Ok(Msg::Event(e, at)) => {
                    if pending.is_empty() {
                        first_ms = at;
                    }
                    pending.push(*e);
                    while let Ok(m) = rx.try_recv() {
                        match m {
                            Msg::Event(e, _) => pending.push(*e),
                            Msg::Stop => stop = true,
                        }
                    }
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => stop = true,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = self.opts.clock.now_ms();
            let due = i64::from(cfg.debounce_secs).saturating_mul(1000);
            if stop
                || pending.len() >= MAX_BATCH
                || (!pending.is_empty() && now.saturating_sub(first_ms) >= due)
            {
                self.enqueue(&cfg, &queue, &status, std::mem::take(&mut pending));
            }
            if stop {
                break;
            }
            let step = self.deliver_one(id, &cfg, &queue, &status);
            // A queue that cannot be written (full disk, read-only dir) is
            // not progress: the same item would be re-sent at once forever.
            let stuck = step == Step::QueueWriteFailed;
            if stuck != queue_stuck {
                queue_stuck = stuck;
                if stuck {
                    tracing::warn!(dest = %id, "webhook queue write failed; retrying on the normal tick");
                } else {
                    tracing::info!(dest = %id, "webhook queue writable again");
                }
            }
            wait = if step == Step::Progressed {
                Duration::ZERO
            } else {
                self.opts.tick
            };
        }
        match self.dest(id) {
            // Not lost on disable/shutdown: kept in the queue for later.
            Some(cfg) if !pending.is_empty() => self.enqueue(&cfg, &queue, &status, pending),
            // Removed destination: nothing may linger on disk.
            None => {
                for it in queue.list() {
                    let _ = queue.ack(&it.id);
                }
            }
            Some(_) => {}
        }
    }

    fn set_error(status: &Mutex<DestStatus>, kind: &str) {
        status.lock().last_error = Some(kind.to_string());
    }

    /// Whether a queue write (ack / bump) took. A failure is surfaced as the
    /// destination's `queue_write` error, which outranks the send's own.
    fn wrote<T>(status: &Mutex<DestStatus>, r: std::io::Result<T>) -> bool {
        let ok = r.is_ok();
        if !ok {
            Self::set_error(status, QUEUE_WRITE_ERROR);
        }
        ok
    }

    /// Chunks `events` into messages and persists each before any send.
    fn enqueue(
        &self,
        cfg: &WebhookDestination,
        queue: &Queue,
        status: &Mutex<DestStatus>,
        events: Vec<DeliveryEvent>,
    ) {
        let events: Vec<DeliveryEvent> = events
            .into_iter()
            .filter(|e| cfg.events.iter().any(|k| k == &e.kind))
            .collect();
        for chunk in events.chunks(MAX_BATCH) {
            let mut stored = Stored {
                events: chunk.to_vec(),
                ..Stored::default()
            };
            if cfg.attach {
                for e in chunk {
                    let (refs, lst) = (self.opts.planner)(e, cfg);
                    for r in refs {
                        if stored.files.iter().any(|f| f.meta.path == r.path) {
                            continue;
                        }
                        match record_file(r) {
                            Ok(f) => stored.files.push(f),
                            Err(l) => stored.listed.push(l),
                        }
                    }
                    stored.listed.extend(lst);
                }
            }
            let Ok(payload) = serde_json::to_value(&stored) else {
                continue;
            };
            let item = QueueItem {
                id: uuid::Uuid::new_v4().simple().to_string(),
                ts_ms: self.opts.clock.now_ms(),
                attempts: 0,
                not_before_ms: 0,
                payload,
            };
            if queue.push(&item).is_err() {
                Self::set_error(status, "queue");
            }
        }
    }

    /// Sends the oldest due queue item, and says whether one was handled
    /// and its outcome recorded in the queue.
    fn deliver_one(
        &self,
        id: &str,
        cfg: &WebhookDestination,
        queue: &Queue,
        status: &Mutex<DestStatus>,
    ) -> Step {
        let now = self.opts.clock.now_ms();
        let Some(item) = queue.next_due(now) else {
            eprintln!(
                "DBG idle now={now} {:?}",
                queue
                    .list()
                    .iter()
                    .map(|i| i.not_before_ms)
                    .collect::<Vec<_>>()
            );
            return Step::Idle;
        };
        let stored: Stored = match serde_json::from_value(item.payload.clone()) {
            Ok(s) => s,
            Err(_) => {
                Self::set_error(status, "bad_item");
                return Step::of(Self::wrote(status, queue.ack(&item.id)));
            }
        };
        let Some(url) = self.secrets.get(id, "url").filter(|u| !u.trim().is_empty()) else {
            return Step::of(self.retry_or_drop(queue, status, &item, None, "no_url"));
        };
        let secret = self.secrets.get(id, "secret").unwrap_or_default();
        let (attached, lst) = load_attachments(&stored, cfg);
        let boundary = format!("ymux-{}", uuid::Uuid::new_v4().simple());
        let ctx = BuildCtx {
            delivery_id: &item.id,
            ts_secs: now / 1000,
            secret: &secret,
            boundary: &boundary,
        };
        let Some(built) = payload::build(cfg, &ctx, &stored.events, &attached, &lst) else {
            return Step::of(Self::wrote(status, queue.ack(&item.id)));
        };
        let out = catch_unwind(AssertUnwindSafe(|| (self.sender)(&url, &built)))
            .unwrap_or_else(|_| SendOutcome::failed(super::client::ErrorKind::Request));
        let verdict = match (out.status, out.error_kind) {
            (Some(s), _) => classify(s),
            (None, Some(k)) if !k.retryable() => Outcome::Drop,
            _ => Outcome::Retry,
        };
        let label = match (out.status, out.error_kind) {
            (Some(s), _) => format!("http_{s}"),
            (None, Some(k)) => k.as_str().to_string(),
            (None, None) => "no_response".to_string(),
        };
        Step::of(match verdict {
            Outcome::Ok => {
                let acked = queue.ack(&item.id).is_ok();
                let mut s = status.lock();
                s.last_ok_at = Some(self.opts.clock.now_ms());
                s.last_error = (!acked).then(|| QUEUE_WRITE_ERROR.to_string());
                acked
            }
            Outcome::Drop => {
                Self::set_error(status, &label);
                Self::wrote(status, queue.ack(&item.id))
            }
            Outcome::Retry => self.retry_or_drop(queue, status, &item, out.retry_after, &label),
        })
    }

    /// Records a failed try (or drops the item once out of attempts).
    /// Returns whether the queue write took.
    fn retry_or_drop(
        &self,
        queue: &Queue,
        status: &Mutex<DestStatus>,
        item: &QueueItem,
        retry_after: Option<Duration>,
        label: &str,
    ) -> bool {
        let jitter = (self.opts.jitter)();
        let r = match next_delay(item.attempts.saturating_add(1), retry_after, jitter) {
            Some(d) => {
                let ms = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
                let at = self.opts.clock.now_ms().saturating_add(ms);
                queue.bump_attempt(&item.id, at).map(drop)
            }
            None => queue.ack(&item.id),
        };
        // Recorded after the queue write, so a status showing this error
        // means the retry is already scheduled (or the item gone).
        Self::set_error(status, label);
        Self::wrote(status, r)
    }
}

/// `DestStatus::last_error` while the destination's queue file cannot be
/// written.
pub const QUEUE_WRITE_ERROR: &str = "queue_write";

/// What one `deliver_one` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing was due.
    Idle,
    /// An item was handled and its outcome is on disk: try the next at once.
    Progressed,
    /// An item was handled but acking or rescheduling it failed, so it is
    /// still due: wait the normal tick rather than spin on it.
    QueueWriteFailed,
}

impl Step {
    fn of(wrote: bool) -> Step {
        if wrote {
            Step::Progressed
        } else {
            Step::QueueWriteFailed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhooks::client::ErrorKind;
    use std::sync::atomic::AtomicI64;
    use std::time::Instant;

    struct FakeClock(AtomicI64);
    impl Clock for FakeClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    impl FakeClock {
        fn advance(&self, secs: i64) {
            self.0.fetch_add(secs * 1000, Ordering::SeqCst);
        }
    }

    type Calls = Arc<Mutex<Vec<(String, Built)>>>;

    struct Rig {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
        secrets: Arc<SecretStore>,
        dests: Arc<RwLock<Vec<WebhookDestination>>>,
        clock: Arc<FakeClock>,
        calls: Calls,
    }

    /// A fixed destination id for a short test name.
    fn uid(name: &str) -> String {
        let mut b = [0u8; 16];
        for (i, c) in name.bytes().take(16).enumerate() {
            b[i] = c;
        }
        b[6] = 0x40;
        b[8] = 0x80;
        uuid::Uuid::from_bytes(b).hyphenated().to_string()
    }

    fn dest(id: &str) -> WebhookDestination {
        WebhookDestination {
            id: uid(id),
            enabled: true,
            debounce_secs: 0,
            ..Default::default()
        }
    }

    fn rig(ids: &[&str]) -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let secrets = Arc::new(SecretStore::new(tmp.path()));
        for id in ids {
            secrets
                .set(&uid(id), "url", &format!("https://{id}.test/hook"))
                .unwrap();
        }
        Rig {
            dir: tmp.path().join("queue"),
            _tmp: tmp,
            secrets,
            dests: Arc::new(RwLock::new(ids.iter().map(|i| dest(i)).collect())),
            clock: Arc::new(FakeClock(AtomicI64::new(1_000_000))),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    impl Rig {
        fn opts(&self) -> Options {
            Options {
                clock: self.clock.clone(),
                tick: Duration::from_millis(5),
                jitter: Arc::new(|| 1.0),
                planner: Arc::new(|_, _| (Vec::new(), Vec::new())),
            }
        }

        /// `script(url, call_no)` decides each outcome.
        fn start(
            &self,
            opts: Options,
            script: impl Fn(&str, usize) -> SendOutcome + Send + Sync + 'static,
        ) -> Dispatcher {
            let calls = self.calls.clone();
            Dispatcher::start_with(
                &self.dir,
                self.secrets.clone(),
                self.dests.clone(),
                move |url, b| {
                    let n = {
                        let mut c = calls.lock();
                        c.push((url.to_string(), b.clone()));
                        c.iter().filter(|(u, _)| u == url).count()
                    };
                    script(url, n)
                },
                opts,
            )
        }

        fn calls_to(&self, host: &str) -> Vec<Built> {
            self.calls
                .lock()
                .iter()
                .filter(|(u, _)| u.contains(host))
                .map(|(_, b)| b.clone())
                .collect()
        }
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for: {what}");
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(80));
    }

    fn ev(n: i64) -> DeliveryEvent {
        DeliveryEvent {
            kind: "turn_end".into(),
            ts: n,
            headline: format!("turn {n}"),
            ..Default::default()
        }
    }

    fn events_in(b: &Built) -> usize {
        let v: Value = serde_json::from_slice(&b.body).unwrap();
        v["events"].as_array().map_or(0, Vec::len)
    }

    #[test]
    fn debounces_then_sends_one_batch() {
        let r = rig(&["a"]);
        r.dests.write()[0].debounce_secs = 10;
        let d = r.start(r.opts(), |_, _| SendOutcome::status(204));
        for i in 0..3 {
            d.publish(&ev(i));
        }
        settle();
        assert!(r.calls_to("a.test").is_empty(), "held by the debounce");
        r.clock.advance(11);
        wait_until("one batch", || r.calls_to("a.test").len() == 1);
        assert_eq!(events_in(&r.calls_to("a.test")[0]), 3);
        wait_until("queue drained", || d.status(&uid("a")).queued == 0);
        assert!(d.status(&uid("a")).last_ok_at.is_some());
        d.shutdown();
    }

    #[test]
    fn a_full_batch_goes_out_without_waiting_and_is_chunked() {
        let r = rig(&["a"]);
        r.dests.write()[0].debounce_secs = 3600;
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        for i in 0..MAX_BATCH as i64 {
            d.publish(&ev(i));
        }
        wait_until("full batch", || !r.calls_to("a.test").is_empty());
        assert_eq!(events_in(&r.calls_to("a.test")[0]), MAX_BATCH);
        d.shutdown();
    }

    #[test]
    fn event_filter_applies() {
        let r = rig(&["a"]);
        r.dests.write()[0].events = vec!["attention".into()];
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        d.publish(&ev(1));
        settle();
        assert!(r.calls_to("a.test").is_empty());
        let mut at = ev(2);
        at.kind = "attention".into();
        d.publish(&at);
        wait_until("attention sent", || r.calls_to("a.test").len() == 1);
        d.shutdown();
    }

    #[test]
    fn retries_with_backoff_on_the_injected_clock() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, n| {
            if n <= 2 {
                SendOutcome::status(503)
            } else {
                SendOutcome::status(200)
            }
        });
        d.publish(&ev(1));
        wait_until("first try", || r.calls_to("a.test").len() == 1);
        settle();
        assert_eq!(r.calls_to("a.test").len(), 1, "waits for the backoff");
        assert_eq!(d.status(&uid("a")).queued, 1);
        assert_eq!(d.status(&uid("a")).last_error.as_deref(), Some("http_503"));
        r.clock.advance(6); // first delay is 5 s at jitter 1.0
        wait_until("second try", || r.calls_to("a.test").len() == 2);
        settle();
        r.clock.advance(6); // second delay is 10 s
        settle();
        assert_eq!(r.calls_to("a.test").len(), 2);
        r.clock.advance(5);
        wait_until("third try ok", || r.calls_to("a.test").len() == 3);
        wait_until("acked", || d.status(&uid("a")).queued == 0);
        assert_eq!(d.status(&uid("a")).last_error, None);
        d.shutdown();
    }

    #[test]
    fn an_unwritable_queue_waits_the_tick_instead_of_resending_at_once() {
        let r = rig(&["a"]);
        // The rewrite's temp path is a directory: every bump_attempt fails,
        // while the first append still works.
        let tmp = r.dir.join(format!("{}.jsonl.tmp", uid("a")));
        std::fs::create_dir_all(&tmp).unwrap();
        let d = r.start(r.opts(), |_, _| SendOutcome::status(503));
        d.publish(&ev(1));
        wait_until("queue_write surfaced", || {
            d.status(&uid("a")).last_error.as_deref() == Some(QUEUE_WRITE_ERROR)
        });
        std::thread::sleep(Duration::from_millis(200));
        // One send per 5 ms tick at most (~40 here), not a hot loop.
        let n = r.calls_to("a.test").len();
        assert!((2..=80).contains(&n), "{n} sends in ~200 ms");
        // Writable again: the retry is scheduled and the backoff holds.
        std::fs::remove_dir(&tmp).unwrap();
        wait_until("backoff recorded", || {
            d.status(&uid("a")).last_error.as_deref() == Some("http_503")
        });
        let n = r.calls_to("a.test").len();
        settle();
        assert_eq!(r.calls_to("a.test").len(), n, "waits for the backoff");
        assert_eq!(d.status(&uid("a")).queued, 1);
        d.shutdown();
    }

    #[test]
    fn a_permanent_4xx_is_dropped_at_once() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, _| SendOutcome::status(404));
        d.publish(&ev(1));
        wait_until("dropped", || {
            r.calls_to("a.test").len() == 1 && d.status(&uid("a")).queued == 0
        });
        assert_eq!(d.status(&uid("a")).last_error.as_deref(), Some("http_404"));
        r.clock.advance(3600);
        settle();
        assert_eq!(r.calls_to("a.test").len(), 1);
        d.shutdown();
    }

    #[test]
    fn a_refused_scheme_is_dropped_not_retried() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, _| SendOutcome::failed(ErrorKind::Scheme));
        d.publish(&ev(1));
        wait_until("dropped", || {
            r.calls_to("a.test").len() == 1 && d.status(&uid("a")).queued == 0
        });
        assert_eq!(d.status(&uid("a")).last_error.as_deref(), Some("scheme"));
        d.shutdown();
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, _| SendOutcome::status(500));
        d.publish(&ev(1));
        for n in 1..=super::super::backoff::MAX_ATTEMPTS as usize {
            wait_until("a try", || r.calls_to("a.test").len() == n);
            settle();
            r.clock.advance(3600);
        }
        wait_until("given up", || d.status(&uid("a")).queued == 0);
        assert_eq!(
            r.calls_to("a.test").len(),
            super::super::backoff::MAX_ATTEMPTS as usize
        );
        d.shutdown();
    }

    #[test]
    fn retry_after_is_honoured() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, n| {
            if n == 1 {
                SendOutcome {
                    status: Some(429),
                    retry_after: Some(Duration::from_secs(120)),
                    error_kind: None,
                }
            } else {
                SendOutcome::status(200)
            }
        });
        d.publish(&ev(1));
        wait_until("first try", || r.calls_to("a.test").len() == 1);
        settle();
        r.clock.advance(100);
        settle();
        assert_eq!(r.calls_to("a.test").len(), 1, "still inside Retry-After");
        r.clock.advance(21);
        wait_until("retried", || r.calls_to("a.test").len() == 2);
        d.shutdown();
    }

    #[test]
    fn queued_items_survive_a_restart() {
        let r = rig(&["a"]);
        let d1 = r.start(r.opts(), |_, _| SendOutcome::status(503));
        d1.publish(&ev(7));
        wait_until("first try", || r.calls_to("a.test").len() == 1);
        d1.shutdown();
        drop(d1);
        let before = r.calls_to("a.test").len();

        r.clock.advance(60);
        let d2 = r.start(r.opts(), |_, _| SendOutcome::status(200));
        wait_until("resumed and sent", || {
            r.calls_to("a.test").len() == before + 1
        });
        wait_until("acked", || d2.status(&uid("a")).queued == 0);
        let last = r.calls_to("a.test").pop().unwrap();
        assert_eq!(events_in(&last), 1);
        d2.shutdown();
    }

    #[test]
    fn pending_events_are_queued_on_shutdown() {
        let r = rig(&["a"]);
        r.dests.write()[0].debounce_secs = 3600;
        let d1 = r.start(r.opts(), |_, _| SendOutcome::status(200));
        d1.publish(&ev(1));
        settle();
        d1.shutdown();
        assert!(r.calls_to("a.test").is_empty());
        let d2 = r.start(r.opts(), |_, _| SendOutcome::status(200));
        wait_until("delivered after restart", || {
            r.calls_to("a.test").len() == 1
        });
        d2.shutdown();
    }

    #[test]
    fn one_failing_destination_does_not_stall_another() {
        let r = rig(&["a", "b"]);
        let d = r.start(r.opts(), |url, _| {
            if url.contains("a.test") {
                SendOutcome::failed(ErrorKind::Connect)
            } else {
                SendOutcome::status(200)
            }
        });
        d.publish(&ev(1));
        wait_until("b delivered", || r.calls_to("b.test").len() == 1);
        wait_until("a tried", || !r.calls_to("a.test").is_empty());
        assert_eq!(d.status(&uid("a")).queued, 1);
        assert_eq!(d.status(&uid("b")).queued, 0);
        assert_eq!(d.status(&uid("a")).last_error.as_deref(), Some("connect"));
        d.shutdown();
    }

    #[test]
    fn disabling_stops_sending_and_enabling_resumes() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        d.publish(&ev(1));
        wait_until("first", || r.calls_to("a.test").len() == 1);
        r.dests.write()[0].enabled = false;
        d.sync();
        settle();
        d.publish(&ev(2));
        settle();
        assert_eq!(r.calls_to("a.test").len(), 1);
        r.dests.write()[0].enabled = true;
        d.publish(&ev(3));
        wait_until("sending again", || r.calls_to("a.test").len() == 2);
        d.shutdown();
    }

    #[test]
    fn a_destination_added_at_runtime_gets_a_worker_and_a_removed_one_is_purged() {
        let r = rig(&["a", "b"]);
        r.dests.write().remove(1);
        let d = r.start(r.opts(), |url, _| {
            if url.contains("b.test") {
                SendOutcome::status(503)
            } else {
                SendOutcome::status(200)
            }
        });
        d.publish(&ev(1));
        wait_until("a", || r.calls_to("a.test").len() == 1);
        assert!(r.calls_to("b.test").is_empty());
        r.dests.write().push(dest("b"));
        d.publish(&ev(2));
        wait_until("b", || !r.calls_to("b.test").is_empty());
        wait_until("b queued", || d.status(&uid("b")).queued == 1);
        r.dests.write().retain(|x| x.id != uid("b"));
        d.sync();
        wait_until("b purged", || d.status(&uid("b")).queued == 0);
        d.shutdown();
    }

    #[test]
    fn publish_never_blocks_and_counts_drops() {
        let r = rig(&["a"]);
        let gate = Arc::new(AtomicBool::new(false));
        let g = gate.clone();
        let d = r.start(r.opts(), move |_, _| {
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            SendOutcome::status(200)
        });
        d.publish(&ev(0));
        wait_until("worker stuck in send", || !r.calls.lock().is_empty());
        let t = Instant::now();
        for i in 0..2000 {
            d.publish(&ev(i));
        }
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(d.dropped() > 0);
        assert_eq!(d.status(&uid("a")).dropped, d.dropped());
        gate.store(true, Ordering::SeqCst);
        d.shutdown();
    }

    #[test]
    fn prompts_never_reach_a_destination_that_does_not_want_them() {
        let r = rig(&["a", "b"]);
        {
            let mut ds = r.dests.write();
            ds[1].include_prompts = true;
            ds[1].detail = "full".into();
        }
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        let mut e = ev(1);
        e.prompt = Some("TOP-SECRET-PROMPT".into());
        d.publish(&e);
        wait_until("both", || {
            !r.calls_to("a.test").is_empty() && !r.calls_to("b.test").is_empty()
        });
        let body = |h: &str| String::from_utf8_lossy(&r.calls_to(h)[0].body).to_string();
        assert!(!body("a.test").contains("TOP-SECRET-PROMPT"));
        assert!(body("b.test").contains("TOP-SECRET-PROMPT"));
        // And not on disk for the one that did not want it.
        let q = std::fs::read_to_string(r.dir.join("a.jsonl")).unwrap_or_default();
        assert!(!q.contains("TOP-SECRET-PROMPT"));
        d.shutdown();
    }

    #[test]
    fn a_missing_url_is_retried_then_dropped_without_a_send() {
        let r = rig(&[]);
        r.dests.write().push(dest("nourl"));
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        d.publish(&ev(1));
        wait_until("queued", || d.status(&uid("nourl")).queued == 1);
        wait_until("no_url recorded", || {
            d.status(&uid("nourl")).last_error.as_deref() == Some("no_url")
        });
        assert!(r.calls.lock().is_empty());
        d.shutdown();
    }

    #[test]
    fn a_panicking_sender_is_a_retryable_failure() {
        let r = rig(&["a"]);
        let d = r.start(r.opts(), |_, n| {
            assert!(n > 1, "boom");
            SendOutcome::status(200)
        });
        d.publish(&ev(1));
        wait_until("tried", || r.calls_to("a.test").len() == 1);
        // The error is recorded once the retry is scheduled, so the clock is
        // not moved before the backoff was computed from it.
        wait_until("retry scheduled", || {
            d.status(&uid("a")).last_error.as_deref() == Some("request")
        });
        assert_eq!(d.status(&uid("a")).queued, 1);
        r.clock.advance(6);
        std::thread::sleep(Duration::from_millis(300));
        eprintln!(
            "DBG calls={} status={:?}",
            r.calls_to("a.test").len(),
            d.status(&uid("a"))
        );
        wait_until("retried ok", || {
            r.calls_to("a.test").len() == 2 && d.status(&uid("a")).queued == 0
        });
        d.shutdown();
    }

    // ---------------------------------------------------------- attachments

    fn attach_rig(file: &Path) -> (Rig, Options) {
        let r = rig(&["a"]);
        {
            let mut ds = r.dests.write();
            ds[0].attach = true;
            ds[0].format = "json".into();
        }
        let planned = planned(file);
        let mut o = r.opts();
        o.planner = Arc::new(move |_, _| (vec![planned.clone()], Vec::new()));
        (r, o)
    }

    /// What the real planner approves for `file` (identity, size, mtime).
    fn planned(file: &Path) -> AttachmentRef {
        let ctx = attach::Ctx {
            root: file.parent().unwrap().to_path_buf(),
            report_dir: String::new(),
            turn_start_ms: 0,
            turn_end_ms: 0,
            allow_types: vec!["txt".into()],
            max_file_bytes: 1 << 20,
            max_total_bytes: 1 << 20,
            max_files: 10,
        };
        let (ok, no) = attach::eligible(&[file.to_string_lossy().into_owned()], &ctx);
        assert!(no.is_empty(), "{no:?}");
        ok.into_iter().next().unwrap()
    }

    #[test]
    fn an_unchanged_file_is_attached() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("report.txt");
        std::fs::write(&f, b"hello").unwrap();
        let (r, o) = attach_rig(&f);
        let d = r.start(o, |_, _| SendOutcome::status(200));
        d.publish(&ev(1));
        wait_until("sent", || r.calls_to("a.test").len() == 1);
        let b = &r.calls_to("a.test")[0];
        assert!(
            b.content_type.starts_with("multipart/"),
            "{}",
            b.content_type
        );
        assert!(String::from_utf8_lossy(&b.body).contains("hello"));
        d.shutdown();
    }

    #[test]
    fn a_file_changed_after_eligibility_is_listed_not_attached() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("report.txt");
        std::fs::write(&f, b"hello").unwrap();
        let (r, o) = attach_rig(&f);
        let n = Arc::new(AtomicU64::new(0));
        let n2 = n.clone();
        // First try fails so we can swap the file before the retry.
        let d = r.start(o, move |_, _| {
            if n2.fetch_add(1, Ordering::SeqCst) == 0 {
                SendOutcome::status(503)
            } else {
                SendOutcome::status(200)
            }
        });
        d.publish(&ev(1));
        wait_until("first try", || r.calls_to("a.test").len() == 1);
        assert!(r.calls_to("a.test")[0]
            .content_type
            .starts_with("multipart/"));
        std::fs::write(&f, b"SWAPPED-CONTENT!").unwrap();
        r.clock.advance(6);
        wait_until("retry", || r.calls_to("a.test").len() == 2);
        let b = &r.calls_to("a.test")[1];
        let body = String::from_utf8_lossy(&b.body).to_string();
        assert!(!body.contains("SWAPPED-CONTENT!"));
        assert!(
            !b.content_type.starts_with("multipart/"),
            "{}",
            b.content_type
        );
        assert!(body.contains("report.txt"), "listed instead: {body}");
        d.shutdown();
    }

    #[test]
    fn load_attachments_enforces_caps_and_lists_vanished_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut stored = Stored::default();
        for i in 0..3 {
            let p = tmp.path().join(format!("f{i}.txt"));
            std::fs::write(&p, b"abc").unwrap();
            stored.files.push(StoredFile { meta: planned(&p) });
        }
        let mut d = dest("a");
        d.max_files = 2;
        let (att, lst) = load_attachments(&stored, &d);
        assert_eq!(att.len(), 2);
        assert_eq!(lst.len(), 1);
        assert_eq!(lst[0].reason, "over the limit");
        // A vanished file is listed.
        std::fs::remove_file(&stored.files[0].meta.path).unwrap();
        d.max_files = 8;
        let (att, lst) = load_attachments(&stored, &d);
        assert_eq!(att.len(), 2);
        assert_eq!(lst[0].reason, "unreadable");
    }

    #[test]
    fn a_destination_with_a_non_uuid_id_is_never_started() {
        let r = rig(&[]);
        for bad in ["../evil", "a", ""] {
            r.dests.write().push(WebhookDestination {
                id: bad.into(),
                enabled: true,
                debounce_secs: 0,
                ..Default::default()
            });
        }
        let d = r.start(r.opts(), |_, _| SendOutcome::status(200));
        d.publish(&ev(1));
        assert!(d.inner.workers.lock().is_empty());
        assert_eq!(d.status("../evil").queued, 0);
        d.shutdown();
        assert!(!r.dir.exists() || std::fs::read_dir(&r.dir).unwrap().next().is_none());
    }

    // -------------------------------------------------------------- mapping

    fn turn_end_record() -> Record {
        let mut r = Record::new(
            1_700_000_000_000,
            kind::TURN_END,
            serde_json::json!({
                "summary": {"headline": "Changed 2 files (+10/-3)", "bullets": ["tests passed"]},
                "files": ["src/a.rs", "src/b.rs"],
                "test": {"runner": "cargo", "command": "cargo test", "passed": true},
                "duration_ms": 5000,
                "title": "api",
                "prompt": "do the thing",
            }),
        );
        r.agent = "claude".into();
        r.account = "work".into();
        r.branch = "feat/x".into();
        r.cwd = "C:\\Git\\proj\\api".into();
        r
    }

    #[test]
    fn turn_end_maps_to_a_delivery_event() {
        let e = event_from_record(&turn_end_record(), "Backend", false).unwrap();
        assert_eq!(e.kind, "turn_end");
        assert_eq!(e.ts, 1_700_000_000_000);
        assert_eq!(e.ws_name, "Backend");
        assert_eq!(e.pane_title, "api");
        assert_eq!(e.agent, "claude");
        assert_eq!(e.account_name, "work");
        assert_eq!(e.branch, "feat/x");
        assert_eq!(e.cwd_display, "proj/api");
        assert_eq!(e.headline, "Changed 2 files (+10/-3)");
        assert_eq!(e.bullets, vec!["tests passed"]);
        assert_eq!(e.files.len(), 2);
        assert_eq!(e.files[0].path, "src/a.rs");
        assert_eq!(
            e.tests,
            Some(TestInfo {
                runner: "cargo".into(),
                passed: Some(true)
            })
        );
        assert_eq!(e.duration_ms, Some(5000));
        assert_eq!(e.prompt, None, "no prompt unless asked");
    }

    #[test]
    fn prompt_is_carried_only_when_asked() {
        let e = event_from_record(&turn_end_record(), "w", true).unwrap();
        assert_eq!(e.prompt.as_deref(), Some("do the thing"));
    }

    #[test]
    fn permission_maps_to_attention() {
        let mut r = Record::new(
            5,
            kind::PERMISSION,
            serde_json::json!({"tool": "Bash", "command": "rm -rf build"}),
        );
        r.agent = "claude".into();
        let e = event_from_record(&r, "w", true).unwrap();
        assert_eq!(e.kind, "attention");
        assert_eq!(e.headline, "Permission requested: Bash");
        assert_eq!(e.bullets, vec!["rm -rf build"]);
        assert_eq!(e.prompt, None);
    }

    #[test]
    fn other_kinds_and_sparse_data_do_not_panic() {
        for k in [kind::PROMPT, kind::TOOL, kind::SUMMARY, "weird"] {
            assert!(event_from_record(&Record::new(1, k, Value::Null), "w", true).is_none());
        }
        let e = event_from_record(&Record::new(1, kind::TURN_END, Value::Null), "w", true).unwrap();
        assert_eq!(e.headline, "Turn finished");
        assert!(e.files.is_empty() && e.tests.is_none());
        let p =
            event_from_record(&Record::new(1, kind::PERMISSION, Value::Null), "w", true).unwrap();
        assert_eq!(p.headline, "Permission requested");
        assert_eq!(cwd_display(""), "");
        assert_eq!(cwd_display("/"), "");
        assert_eq!(cwd_display("/a"), "a");
    }
}
