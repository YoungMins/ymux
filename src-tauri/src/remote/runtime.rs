//! Desktop glue for the remote link (CLAUDE.md rule 19): the socket thread,
//! the managed state and the Tauri commands. Every decision lives in the pure
//! `engine`; this file moves bytes and events.
//!
//! One named thread owns the WebSocket. It reads with a 200 ms timeout so the
//! same loop also pumps the engine's outbox and timers; commands never touch
//! the socket, they change the engine or the config and wake the thread.
//! Lock order is the device-set revision (`devset`), then the engine, then
//! (briefly) config; nothing is emitted or sent with the engine locked. Every
//! change to the paired devices or their capabilities runs inside
//! `devset.mutate`, and the worker installs a snapshot of them only while the
//! revision it read at still stands (see `devset.rs`).

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::Thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Request;
use tauri::{AppHandle, Emitter, Manager, State, Webview};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, Message, WebSocket};

use super::audit::Audit;
use super::backoff;
use super::devset::{switch_server, DeviceSet, SwitchSteps};
use super::engine::{Engine, Out, PairOutcome};
use super::keys::{random_unit, Keypair, LinkKey};
use super::proto::{Cap, MAX_FRAME};
use super::{
    connection_allowed, normalize_server_url, now_ms, pair_link, server_origin, ws_url, DEV_ENV,
};
use crate::commands::AppState;
use crate::config::model::{RemoteConfig, RemoteDevice};
use crate::fspath::guard_local;
use crate::secrets::SecretStore;
use crate::{YmuxError, YmuxResult};

/// Secrets-store id. Fields: `secret` = the private key, `api_key` = the
/// setup token. Not a webhook UUID, so the webhook prune never touches it.
pub const SECRET_ID: &str = "remote";

/// Secrets-store id of one device's link key (field `secret`). The store's
/// field names are fixed and ids are free strings, so there is one id per
/// device; it is removed with the device.
pub fn link_secret_id(device_id: &str) -> String {
    format!("remote-dev:{device_id}")
}

pub const STATUS_EVENT: &str = "remote://status";
pub const REQUEST_EVENT: &str = "remote://request";
pub const PAIR_EVENT: &str = "remote://pair-request";
/// Two devices used one pairing code; no payload.
pub const PAIR_CONFLICT_EVENT: &str = "remote://pair-conflict";

const MAIN: &str = "main";
/// Socket read timeout: also the pump interval.
const POLL: Duration = Duration::from_millis(200);
/// No frame (not even the server's 30 s ping) for this long: the link is dead.
const SILENCE_MS: u64 = 90_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn cfg_err(msg: impl Into<String>) -> YmuxError {
    YmuxError::Config(msg.into())
}

// ------------------------------------------------------------------ state

#[derive(Debug, Clone)]
struct Link {
    /// `connecting`, `online` or `error` (`off` / `unavailable` are derived).
    state: &'static str,
    error: String,
}

/// What the socket loop needs from the application around it: the config, the
/// events for the page, and the two config writes the link makes itself. The
/// Tauri app is the real one (`TauriHost`); the live end-to-end test supplies
/// a small in-memory one, so the real loop and engine run without a window.
trait Host: Send + Sync {
    fn cfg(&self) -> RemoteConfig;
    /// `remote://status`.
    fn emit_status(&self);
    /// `remote://request`, to the main window.
    fn emit_request(&self, payload: Value);
    /// `remote://pair-request`, to the main window.
    fn emit_pair(&self, payload: Value);
    /// `remote://pair-conflict`, to the main window.
    fn emit_pair_conflict(&self);
    /// Remember a freshly minted desktop id (and save).
    fn set_desktop_id(&self, id: &str);
    /// Remember that the server at `ws_url_of_connection` accepted this
    /// desktop (and save, once); ignored when the configured server is no
    /// longer that one.
    fn mark_registered(&self, ws_url_of_connection: &str);
    /// Forget paired devices whose link key is gone (and save).
    fn drop_devices(&self, ids: &[String]);
    /// Start clean under a new desktop id: not registered, no paired devices
    /// (server address, name and notify settings stay). `false` = not saved.
    fn reset_identity(&self, new_desktop_id: &str) -> bool;
}

struct TauriHost {
    app: AppHandle,
}

impl Host for TauriHost {
    fn cfg(&self) -> RemoteConfig {
        self.app.state::<AppState>().config.snapshot().remote
    }

    fn emit_status(&self) {
        let _ = self.app.emit(STATUS_EVENT, ());
    }

    fn emit_request(&self, payload: Value) {
        let _ = self.app.emit_to(MAIN, REQUEST_EVENT, payload);
    }

    fn emit_pair(&self, payload: Value) {
        let _ = self.app.emit_to(MAIN, PAIR_EVENT, payload);
    }

    fn emit_pair_conflict(&self) {
        let _ = self.app.emit_to(MAIN, PAIR_CONFLICT_EVENT, ());
    }

    fn drop_devices(&self, ids: &[String]) {
        let state = self.app.state::<AppState>();
        state
            .config
            .update(|c| c.remote.devices.retain(|d| !ids.contains(&d.id)));
        if let Err(e) = state.config.flush() {
            tracing::warn!(error = %e, "could not save the remote device list");
        }
    }

    fn reset_identity(&self, new_desktop_id: &str) -> bool {
        let state = self.app.state::<AppState>();
        state.config.update(|c| {
            c.remote.desktop_id = new_desktop_id.to_string();
            c.remote.registered = false;
            c.remote.devices.clear();
        });
        state.config.flush().is_ok()
    }

    fn set_desktop_id(&self, id: &str) {
        let state = self.app.state::<AppState>();
        state
            .config
            .update(|c| c.remote.desktop_id = id.to_string());
        let _ = state.config.flush();
    }

    fn mark_registered(&self, ws_url_of_connection: &str) {
        let state = self.app.state::<AppState>();
        // Under the config lock: a `ready` from a connection to a server the
        // user has since switched away from must not register the new one.
        let was = state.config.update(|c| {
            if super::ws_url(&c.remote.server_url).as_deref() != Ok(ws_url_of_connection) {
                return true;
            }
            let was = c.remote.registered;
            c.remote.registered = true;
            was
        });
        if !was {
            if let Err(e) = state.config.flush() {
                tracing::warn!(error = %e, "could not save the remote registration");
            }
        }
    }
}

/// The configured devices, by what the secret store said about each link key.
#[derive(Default)]
struct PairedSet {
    usable: Vec<(RemoteDevice, LinkKey)>,
    /// The store answered and there is no (valid) key: forget the device.
    missing: Vec<String>,
    /// The store could not be read: keep the device, do not use it this run.
    unreadable: Vec<String>,
}

/// What the worker read to build or refresh the engine, all at one revision
/// of the device set.
struct Snapshot {
    desktop_id: String,
    name: String,
    origin: String,
    token: String,
    notify_waiting: bool,
    notify_done: bool,
    set: PairedSet,
}

struct Shared {
    host: Arc<dyn Host>,
    /// Devices kept but unusable since the last `prepare` (store unreadable).
    unreadable: Mutex<Vec<String>>,
    /// `identity_reset` after the identity key was found gone and the pairing
    /// state was started afresh; cleared by the next approved pairing or a
    /// new setup token. Memory only: the config is already clean by then.
    notice: Mutex<&'static str>,
    secrets: Arc<SecretStore>,
    audit: Audit,
    /// Debug build without the opt-in, or a stand-down instance.
    allowed: bool,
    /// The revision of the paired-device set (outside the engine, which may
    /// not exist while the link is off).
    devset: DeviceSet,
    engine: Mutex<Option<Engine>>,
    link: Mutex<Link>,
    /// Bumped by every command that changes what the worker should do.
    gen: AtomicU64,
    worker: Mutex<Option<Thread>>,
    /// The newest pane list the page published, kept so a fresh engine can be
    /// seeded without waiting for the next change.
    latest_panes: Mutex<Value>,
    /// `allowed` and the config says enabled: lets `remote_publish_panes`
    /// return without touching the config.
    active: AtomicBool,
}

/// Managed state.
pub struct RemoteRuntime {
    shared: Arc<Shared>,
}

impl Shared {
    fn new(
        host: Arc<dyn Host>,
        secrets: Arc<SecretStore>,
        audit: Audit,
        allowed: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            unreadable: Mutex::new(Vec::new()),
            notice: Mutex::new(""),
            secrets,
            audit,
            allowed,
            devset: DeviceSet::default(),
            engine: Mutex::new(None),
            link: Mutex::new(Link {
                state: "connecting",
                error: String::new(),
            }),
            gen: AtomicU64::new(0),
            worker: Mutex::new(None),
            latest_panes: Mutex::new(Value::Null),
            active: AtomicBool::new(false),
        })
    }

    fn cfg(&self) -> RemoteConfig {
        self.host.cfg()
    }

    fn changed(&self) {
        self.host.emit_status();
    }

    fn wake(&self) {
        self.gen.fetch_add(1, Ordering::SeqCst);
        if let Some(t) = self.worker.lock().as_ref() {
            t.unpark();
        }
    }

    fn set_link(&self, state: &'static str, error: &str) {
        let mut l = self.link.lock();
        if l.state != state || l.error != error {
            l.state = state;
            l.error = error.to_string();
            drop(l);
            self.changed();
        }
    }

    fn with_engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        self.engine.lock().as_mut().map(f)
    }

    /// Act on what the engine produced. `ws` is `None` when no socket is up
    /// (frames are then dropped). Returns the code of a requested close.
    fn handle(
        &self,
        outs: Vec<Out>,
        mut ws: Option<&mut Ws>,
        url: Option<&str>,
    ) -> Result<bool, String> {
        let mut ready = false;
        let mut dirty = false;
        for o in outs {
            match o {
                Out::Send(frame) => {
                    if let Some(w) = ws.as_deref_mut() {
                        w.send(Message::text(frame))
                            .map_err(|e| ws_error_code(&e).to_string())?;
                    }
                }
                Out::Request { payload, .. } => {
                    self.host.emit_request(payload);
                }
                Out::PairRequest { device_id, name } => {
                    // No check number: the user types the one the phone shows.
                    self.host
                        .emit_pair(serde_json::json!({"device_id": device_id, "name": name}));
                }
                Out::PairConflict => self.host.emit_pair_conflict(),
                Out::Audit(entry) => self.audit.record(&entry),
                Out::Changed => dirty = true,
                Out::Ready => {
                    ready = true;
                    self.mark_registered(url);
                }
                Out::Close(code) => return Err(code),
            }
        }
        if dirty {
            self.changed();
        }
        Ok(ready)
    }

    /// Drain and act on the engine's outbox with no socket (idle waits).
    fn pump(&self) {
        let outs = self.with_engine(Engine::drain).unwrap_or_default();
        if !outs.is_empty() {
            let _ = self.handle(outs, None, None);
        }
    }

    fn mark_registered(&self, url: Option<&str>) {
        if let Some(url) = url {
            self.host.mark_registered(url);
        }
    }

    /// Sleep until `dur` passes or the generation moves; keeps the engine's
    /// outbox moving. `true` = woken by a change.
    fn wait(&self, dur: Duration, gen: u64) -> bool {
        let end = std::time::Instant::now() + dur;
        loop {
            self.pump();
            if self.gen.load(Ordering::SeqCst) != gen {
                return true;
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            std::thread::park_timeout(left.min(Duration::from_millis(500)));
        }
    }

    // ------------------------------------------------------------ worker

    fn load_keys(&self) -> Result<Keypair, &'static str> {
        // The checked read: a store that cannot be READ must not look like a
        // store with no identity, or a new key would be minted over the old
        // one (and every pairing lost). Mint only on a definite "not present".
        match self
            .secrets
            .get_checked(SECRET_ID, "secret")
            .map_err(|_| "secret_store")?
        {
            Some(s) => Keypair::from_b64u(&s).ok_or("bad_key"),
            None => {
                let k = Keypair::generate();
                self.secrets
                    .set_checked(SECRET_ID, "secret", &k.secret_b64u())
                    .map_err(|_| "secret_store")?;
                Ok(k)
            }
        }
    }

    /// The identity key is absent from a store that can be read, yet the
    /// config says this desktop was registered or has paired phones: the key
    /// (and with it every link key) was lost. A new key under the old
    /// `desktop_id` is refused by the server forever (`key_mismatch`), so
    /// start clean instead: a new id, not registered, no devices and no link
    /// keys. The server address, name and notify settings stay.
    /// `Ok(())` also when there was nothing to do.
    fn reset_identity_if_lost(&self) -> Result<(), &'static str> {
        let cfg = self.cfg();
        if !super::proto::is_uuid(&cfg.desktop_id) || !(cfg.registered || !cfg.devices.is_empty()) {
            return Ok(());
        }
        if self.engine.lock().is_some() {
            return Ok(());
        }
        if self
            .secrets
            .get_checked(SECRET_ID, "secret")
            .map_err(|_| "secret_store")?
            .is_some()
        {
            return Ok(());
        }
        self.devset.mutate(|| -> Result<(), &'static str> {
            for d in &cfg.devices {
                self.secrets
                    .set_checked(&link_secret_id(&d.id), "secret", "")
                    .map_err(|_| "secret_store")?;
            }
            if !self.host.reset_identity(&uuid::Uuid::new_v4().to_string()) {
                return Err("config");
            }
            *self.notice.lock() = "identity_reset";
            tracing::warn!("remote identity key is gone: starting with a new desktop id");
            Ok(())
        })?;
        self.changed();
        Ok(())
    }

    /// Sort the configured devices by what the secret store says about their
    /// link key. A device without one could only talk unsealed, so it is not
    /// usable; but "the store answered: no key" (the device is forgotten) is
    /// not "the store could not be read" (the device is kept, unused this run).
    fn paired(&self, devices: &[RemoteDevice]) -> PairedSet {
        let mut set = PairedSet::default();
        for d in devices {
            match self.secrets.get_checked(&link_secret_id(&d.id), "secret") {
                Ok(Some(s)) => match LinkKey::from_b64u(&s) {
                    Some(key) => set.usable.push((d.clone(), key)),
                    None => set.missing.push(d.id.clone()),
                },
                Ok(None) => set.missing.push(d.id.clone()),
                Err(_) => set.unreadable.push(d.id.clone()),
            }
        }
        set
    }

    /// Make sure an engine exists and matches the config. A device in the
    /// config with no stored link key is dropped (and the server told). The
    /// config and the link keys are read at one revision of the device set
    /// and installed only if no settings command changed the set meanwhile
    /// (else they are read again), so an old list never revives a removed
    /// device or a higher capability.
    fn prepare(&self) -> Result<(), &'static str> {
        self.devset
            .refresh(
                || self.snapshot(),
                |snap| snap.and_then(|s| self.install(s)),
            )
            .unwrap_or(Err("busy"))
    }

    fn snapshot(&self) -> Result<Snapshot, &'static str> {
        self.reset_identity_if_lost()?;
        let mut cfg = self.cfg();
        if !super::proto::is_uuid(&cfg.desktop_id) {
            self.host.set_desktop_id(&uuid::Uuid::new_v4().to_string());
            cfg = self.cfg();
        }
        let origin = server_origin(&cfg.server_url).map_err(|_| "url")?;
        // The setup token goes out only until the server accepted this
        // desktop once.
        let token = if cfg.registered {
            String::new()
        } else {
            self.secrets.get(SECRET_ID, "api_key").unwrap_or_default()
        };
        Ok(Snapshot {
            desktop_id: cfg.desktop_id.clone(),
            name: cfg.effective_name(),
            notify_waiting: cfg.notify_waiting,
            notify_done: cfg.notify_done,
            set: self.paired(&cfg.devices),
            origin,
            token,
        })
    }

    fn install(&self, snap: Snapshot) -> Result<(), &'static str> {
        let Snapshot {
            desktop_id,
            name,
            origin,
            token,
            notify_waiting,
            notify_done,
            set:
                PairedSet {
                    usable: paired,
                    missing: orphans,
                    unreadable,
                },
        } = snap;
        if !unreadable.is_empty() {
            tracing::warn!(
                count = unreadable.len(),
                "remote devices kept but unusable: the secret store could not be read"
            );
        }
        *self.unreadable.lock() = unreadable.clone();
        if !orphans.is_empty() {
            tracing::warn!(
                count = orphans.len(),
                "dropping remote devices without a link key"
            );
            self.host.drop_devices(&orphans);
        }
        let mut guard = self.engine.lock();
        if guard.is_none() {
            let keys = self.load_keys()?;
            let mut e = Engine::new(&desktop_id, &name, keys);
            e.set_origin(&origin);
            e.set_identity(&name, &token);
            e.set_notify(notify_waiting, notify_done);
            e.set_devices(paired);
            e.set_unusable(unreadable);
            let panes = self.latest_panes.lock().clone();
            if !panes.is_null() {
                e.publish_panes(&panes, now_ms());
            }
            *guard = Some(e);
        } else if let Some(e) = guard.as_mut() {
            e.set_origin(&origin);
            e.set_identity(&name, &token);
            e.set_notify(notify_waiting, notify_done);
            e.set_devices(paired);
            e.set_unusable(unreadable);
        }
        if let Some(e) = guard.as_mut() {
            for id in &orphans {
                e.device_removed(id, now_ms());
            }
        }
        Ok(())
    }

    fn run(self: Arc<Self>) {
        let mut attempt: u32 = 0;
        loop {
            let gen = self.gen.load(Ordering::SeqCst);
            let cfg = self.cfg();
            let url = match (cfg.enabled, ws_url(&cfg.server_url)) {
                (true, Ok(u)) => u,
                _ => {
                    *self.engine.lock() = None;
                    self.set_link("connecting", "");
                    attempt = 0;
                    self.wait(Duration::from_secs(3600), gen);
                    continue;
                }
            };
            if let Err(code) = self.prepare() {
                self.set_link("error", code);
                attempt = attempt.saturating_add(1);
                self.wait(backoff::delay(attempt, random_unit()), gen);
                continue;
            }
            self.set_link("connecting", "");
            match connect(&url) {
                Ok(ws) => {
                    let end = self.session(ws, &url, gen);
                    if end.was_ready {
                        attempt = 0;
                    }
                    if end.reconfigured {
                        continue;
                    }
                    self.set_link("error", &end.code);
                }
                Err(code) => self.set_link("error", &code),
            }
            let wait = backoff::delay(attempt, random_unit());
            attempt = attempt.saturating_add(1);
            if self.wait(wait, gen) {
                attempt = 0;
            }
        }
    }

    /// One connection, from the challenge to its end.
    fn session(&self, mut ws: Ws, url: &str, gen: u64) -> SessionEnd {
        set_read_timeout(&mut ws, POLL);
        self.with_engine(Engine::on_connect);
        let mut end = SessionEnd {
            code: "closed".into(),
            was_ready: false,
            reconfigured: false,
        };
        let mut last_rx = now_ms();
        let mut seen_gen = gen;
        loop {
            if self.gen.load(Ordering::SeqCst) != seen_gen {
                seen_gen = self.gen.load(Ordering::SeqCst);
                let cfg = self.cfg();
                let same = cfg.enabled && ws_url(&cfg.server_url).as_deref() == Ok(url);
                if !same {
                    end.reconfigured = true;
                    break;
                }
                if self.prepare().is_err() {
                    end.code = "secret_store".into();
                    break;
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => {
                    last_rx = now_ms();
                    self.with_engine(|e| e.on_text(t.as_str(), now_ms()));
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => last_rx = now_ms(),
                Ok(Message::Close(_)) => {
                    end.code = "closed".into();
                    break;
                }
                Ok(_) => {
                    end.code = "binary_frame".into();
                    break;
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => {
                    end.code = ws_error_code(&e).into();
                    break;
                }
            }
            let outs = self
                .with_engine(|e| {
                    e.tick(now_ms());
                    e.drain()
                })
                .unwrap_or_default();
            match self.handle(outs, Some(&mut ws), Some(url)) {
                Ok(ready) => {
                    if ready {
                        end.was_ready = true;
                        self.set_link("online", "");
                    }
                }
                Err(code) => {
                    end.code = code;
                    break;
                }
            }
            if now_ms().saturating_sub(last_rx) > SILENCE_MS {
                end.code = "timeout".into();
                break;
            }
        }
        let _ = ws.close(None);
        let _ = ws.flush();
        self.with_engine(|e| e.on_disconnect(now_ms()));
        self.pump();
        end
    }
}

struct SessionEnd {
    code: String,
    was_ready: bool,
    reconfigured: bool,
}

/// A short, fixed word for a socket error. Never the error text: it can carry
/// the URL.
fn ws_error_code(e: &tungstenite::Error) -> &'static str {
    match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => "closed",
        tungstenite::Error::Io(_) => "io",
        tungstenite::Error::Tls(_) => "tls",
        tungstenite::Error::Capacity(_) => "frame_too_large",
        tungstenite::Error::Protocol(_) | tungstenite::Error::Utf8(_) => "protocol",
        _ => "socket",
    }
}

// -------------------------------------------------------------- transport

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| "tls".to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Change the read timeout of the socket the WebSocket owns. It must be set
/// on that very handle: on Windows `SO_RCVTIMEO` belongs to the descriptor, so
/// a `try_clone` of the stream does not carry it over and the loop would block
/// for the old (10 s connect) timeout instead of its 200 ms poll.
fn set_read_timeout(ws: &mut Ws, d: Duration) {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(d));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_read_timeout(Some(d));
        }
        _ => {}
    }
}

/// TCP + TLS + WebSocket handshake. No `Origin` header, no redirects (a
/// non-101 answer is an error), frames capped at 256 KiB. The error is a
/// short fixed code.
fn connect(url: &str) -> Result<Ws, String> {
    let parsed = url::Url::parse(url).map_err(|_| "url".to_string())?;
    let host = parsed.host_str().ok_or("url")?.to_string();
    let port = parsed.port_or_known_default().ok_or("url")?;
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| "dns".to_string())?;
    let mut stream = None;
    for a in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            stream = Some(s);
            break;
        }
    }
    let stream = stream.ok_or_else(|| "connect".to_string())?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let connector = if parsed.scheme() == "wss" {
        Connector::Rustls(tls_config()?)
    } else {
        Connector::Plain
    };
    let cfg = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME));
    match tungstenite::client_tls_with_config(url, stream, Some(cfg), Some(connector)) {
        Ok((ws, _resp)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))) => {
            Err(format!("http_{}", resp.status().as_u16()))
        }
        Err(tungstenite::HandshakeError::Failure(e)) => Err(ws_error_code(&e).to_string()),
        Err(tungstenite::HandshakeError::Interrupted(_)) => Err("timeout".to_string()),
    }
}

// ------------------------------------------------------------------ start

/// Create the managed state and, where connecting is allowed at all, the
/// worker thread. Call once in `setup`, after `AgentHookPort` is managed:
/// `standdown` is "this instance has no hook receiver".
pub fn start(app: &AppHandle, standdown: bool) {
    let dir = ytheme::config_dir().unwrap_or_else(|| std::path::PathBuf::from("./ymux-config"));
    // The webhook runtime's store when there is one: two stores on one file
    // would race each other's read-modify-write.
    let secrets = app
        .try_state::<crate::webhooks::runtime::WebhookRuntime>()
        .map(|w| w.secrets.clone())
        .unwrap_or_else(|| Arc::new(SecretStore::new(&dir)));
    let dev = std::env::var(DEV_ENV).ok();
    let allowed = connection_allowed(cfg!(debug_assertions), dev.as_deref(), standdown);
    let shared = Shared::new(
        Arc::new(TauriHost { app: app.clone() }),
        secrets,
        Audit::new(dir.join("remote").join("audit.jsonl")),
        allowed,
    );
    let enabled = app.state::<AppState>().config.snapshot().remote.enabled;
    shared.active.store(allowed && enabled, Ordering::Relaxed);
    if allowed {
        let worker = shared.clone();
        match std::thread::Builder::new()
            .name("ymux-remote".into())
            .spawn(move || worker.run())
        {
            Ok(h) => *shared.worker.lock() = Some(h.thread().clone()),
            Err(e) => tracing::warn!(error = %e, "could not start the remote link thread"),
        }
    } else {
        tracing::info!("remote link unavailable in this instance");
    }
    app.manage(RemoteRuntime { shared });
}

// --------------------------------------------------------------- commands

#[derive(Debug, Serialize)]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    pub cap: String,
    pub added_ms: u64,
    pub online: bool,
    /// `secret_store` while the device's link key could not be read (the
    /// device is kept and retried), else empty.
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct PairingInfo {
    pub url: String,
    pub expires_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub server_url: String,
    pub name: String,
    pub registered: bool,
    pub has_token: bool,
    /// `off`, `connecting`, `online`, `error` or `unavailable`.
    pub state: String,
    /// A short code, or empty.
    pub error: String,
    /// `identity_reset` (the identity key was lost: enter the setup token if
    /// asked and pair the phones again), or empty.
    pub notice: String,
    pub notify_waiting: bool,
    pub notify_done: bool,
    pub devices: Vec<DeviceStatus>,
    pub pairing: Option<PairingInfo>,
}

fn status_of(sh: &Shared) -> RemoteStatus {
    let cfg = sh.cfg();
    let secret = sh.secrets.status(SECRET_ID);
    let (online, window) = sh
        .with_engine(|e| (e.online_devices(), e.pairing_window()))
        .unwrap_or_default();
    let link = sh.link.lock().clone();
    let unreadable = sh.unreadable.lock().clone();
    let (state, error) = if !sh.allowed {
        ("unavailable", String::new())
    } else if !cfg.enabled {
        ("off", String::new())
    } else {
        (link.state, link.error)
    };
    RemoteStatus {
        enabled: cfg.enabled,
        name: cfg.effective_name(),
        registered: cfg.registered && secret.secret,
        has_token: secret.api_key,
        state: state.to_string(),
        error,
        notice: (*sh.notice.lock()).to_string(),
        notify_waiting: cfg.notify_waiting,
        notify_done: cfg.notify_done,
        devices: cfg
            .devices
            .iter()
            .map(|d| DeviceStatus {
                id: d.id.clone(),
                name: d.name.clone(),
                cap: d.cap.clone(),
                added_ms: d.added_ms,
                online: online.contains(&d.id),
                error: if unreadable.contains(&d.id) {
                    "secret_store".into()
                } else {
                    String::new()
                },
            })
            .collect(),
        pairing: window.map(|(code, dk, expires_ms)| PairingInfo {
            url: pair_link(&cfg.server_url, &cfg.desktop_id, &code, &dk),
            expires_ms,
        }),
        server_url: cfg.server_url,
    }
}

/// Persist a change to `remote` and flush.
fn update_remote<R>(state: &AppState, f: impl FnOnce(&mut RemoteConfig) -> R) -> YmuxResult<R> {
    let r = state.config.update(|c| f(&mut c.remote));
    state.config.flush()?;
    Ok(r)
}

#[tauri::command(async)]
pub fn remote_status(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
) -> YmuxResult<RemoteStatus> {
    guard_local(&webview, &request, "remote_status")?;
    Ok(status_of(&rt.shared))
}

/// Validate, persist and (re)start or stop the link. Pointing it at a
/// different server forgets the registration and the paired phones (they
/// belong to a server); the status then carries `error = "server_changed"`
/// until the next connection attempt.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn remote_configure(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    state: State<'_, AppState>,
    server_url: String,
    name: String,
    enabled: bool,
    notify_waiting: bool,
    notify_done: bool,
) -> YmuxResult<RemoteStatus> {
    guard_local(&webview, &request, "remote_configure")?;
    let sh = &rt.shared;
    let url = if server_url.trim().is_empty() {
        String::new()
    } else {
        normalize_server_url(&server_url).map_err(|e| cfg_err(e.message()))?
    };
    if enabled && url.is_empty() {
        return Err(cfg_err("enter the server address first"));
    }
    let name = super::clip::text(name.trim(), 60);
    let apply = |r: &mut RemoteConfig| {
        if r.server_url != url {
            r.registered = false;
            r.devices.clear();
        }
        if !super::proto::is_uuid(&r.desktop_id) {
            r.desktop_id = uuid::Uuid::new_v4().to_string();
        }
        r.server_url = url.clone();
        r.name = name.clone();
        r.enabled = enabled;
        r.notify_waiting = notify_waiting;
        r.notify_done = notify_done;
    };
    // Settings changes and the worker's reads of the device set take turns
    // (see `devset.rs`). Pointing the link at another server makes the setup
    // token and the link keys the old server's to forget: they are cleared
    // BEFORE the new address is saved, so a worker that wakes in between
    // cannot send the old token to the new server. If one cannot be cleared
    // the address is left as it was and the call fails.
    let server_changed = sh.devset.mutate(|| -> YmuxResult<bool> {
        let cur = sh.cfg();
        let changed = cur.server_url != url;
        if changed {
            let forgotten: Vec<String> = cur.devices.iter().map(|d| d.id.clone()).collect();
            let mut steps = ServerSwitch {
                secrets: &sh.secrets,
                state: &state,
                apply: &apply,
            };
            switch_server(&mut steps, &forgotten).map_err(|e| cfg_err(e.message()))?;
            sh.with_engine(|e| {
                e.pair_cancel();
                e.set_devices(Vec::new());
            });
        } else {
            update_remote(&state, apply)?;
        }
        Ok(changed)
    })?;
    sh.active.store(sh.allowed && enabled, Ordering::Relaxed);
    sh.wake();
    if server_changed && !server_url.trim().is_empty() {
        sh.set_link("connecting", "server_changed");
    }
    sh.changed();
    Ok(status_of(sh))
}

/// The steps of switching server, for `devset::switch_server`: secrets and
/// config, in the order that function runs them.
struct ServerSwitch<'a> {
    secrets: &'a SecretStore,
    state: &'a AppState,
    apply: &'a dyn Fn(&mut RemoteConfig),
}

impl SwitchSteps for ServerSwitch<'_> {
    fn clear_setup_token(&mut self) -> bool {
        match self.secrets.set_checked(SECRET_ID, "api_key", "") {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "could not clear the remote setup token");
                false
            }
        }
    }

    fn clear_link_key(&mut self, device_id: &str) -> bool {
        match self
            .secrets
            .set_checked(&link_secret_id(device_id), "secret", "")
        {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "could not remove a remote link key");
                false
            }
        }
    }

    fn store_config(&mut self) -> bool {
        update_remote(self.state, |r| (self.apply)(r)).is_ok()
    }
}

/// Write-only: store (or, empty, clear) the server's setup token.
#[tauri::command(async)]
pub fn remote_set_token(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    token: String,
) -> YmuxResult<RemoteStatus> {
    guard_local(&webview, &request, "remote_set_token")?;
    let sh = &rt.shared;
    sh.secrets.set_checked(SECRET_ID, "api_key", token.trim())?;
    if !token.trim().is_empty() {
        *sh.notice.lock() = "";
    }
    sh.wake();
    sh.changed();
    Ok(status_of(sh))
}

#[derive(Debug, Serialize)]
pub struct PairStart {
    pub url: String,
    pub expires_ms: u64,
}

#[tauri::command(async)]
pub fn remote_pair_start(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
) -> YmuxResult<PairStart> {
    guard_local(&webview, &request, "remote_pair_start")?;
    let sh = &rt.shared;
    let cfg = sh.cfg();
    let opened = sh
        .with_engine(|e| e.pair_open(now_ms()))
        .unwrap_or(Err("offline"));
    let (code, dk, expires_ms) =
        opened.map_err(|_| cfg_err("connect to the server first (the status must be online)"))?;
    sh.changed();
    Ok(PairStart {
        url: pair_link(&cfg.server_url, &cfg.desktop_id, &code, &dk),
        expires_ms,
    })
}

#[tauri::command(async)]
pub fn remote_pair_cancel(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "remote_pair_cancel")?;
    rt.shared.with_engine(Engine::pair_cancel);
    rt.shared.changed();
    Ok(())
}

/// What the user's answer to a pairing request did, for the dialog.
#[derive(Debug, Serialize)]
pub struct PairDecision {
    /// `approved`, `rejected`, `wrong` (try again) or `refused` (too many
    /// wrong numbers: the request is gone and the window closed).
    pub result: &'static str,
    /// Wrong numbers the request may still have (0 unless `result` is `wrong`).
    pub tries_left: u8,
    pub status: RemoteStatus,
}

/// `check` is the six digits the user typed from the phone's screen; it is
/// compared in the engine and never stored or logged here.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn remote_pair_decide(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    state: State<'_, AppState>,
    device_id: String,
    approve: bool,
    cap: String,
    check: String,
) -> YmuxResult<PairDecision> {
    guard_local(&webview, &request, "remote_pair_decide")?;
    let sh = &rt.shared;
    let outcome = sh.devset.mutate(|| -> YmuxResult<PairOutcome> {
        let mut guard = sh.engine.lock();
        let engine = guard
            .as_mut()
            .ok_or_else(|| cfg_err("the link is not running"))?;
        // Approved: the link key goes to the secrets store first, then the
        // device to the config, before the engine trusts either; a failure of
        // either refuses the phone.
        let mut persist = |device: &RemoteDevice, link: &LinkKey| -> bool {
            let id = link_secret_id(&device.id);
            if sh
                .secrets
                .set_checked(&id, "secret", &link.to_b64u())
                .is_err()
            {
                return false;
            }
            let saved = update_remote(&state, |r| {
                r.devices.retain(|d| d.id != device.id);
                r.devices.push(device.clone());
            });
            if saved.is_err() {
                let _ = sh.secrets.set_checked(&id, "secret", "");
            }
            saved.is_ok()
        };
        engine
            .pair_decide(&device_id, approve, &cap, &check, now_ms(), &mut persist)
            .map_err(|code| cfg_err(format!("pairing: {code}")))
    })?;
    sh.changed();
    let (result, tries_left) = match outcome {
        PairOutcome::Approved(_) => {
            *sh.notice.lock() = "";
            ("approved", 0)
        }
        PairOutcome::Rejected => ("rejected", 0),
        PairOutcome::Wrong { tries_left } => ("wrong", tries_left),
        PairOutcome::Refused => ("refused", 0),
    };
    Ok(PairDecision {
        result,
        tries_left,
        status: status_of(sh),
    })
}

#[tauri::command(async)]
pub fn remote_device_set_cap(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    state: State<'_, AppState>,
    device_id: String,
    cap: String,
) -> YmuxResult<RemoteStatus> {
    guard_local(&webview, &request, "remote_device_set_cap")?;
    let sh = &rt.shared;
    let cap = Cap::parse(&cap).ok_or_else(|| cfg_err("unknown capability"))?;
    sh.devset.mutate(|| -> YmuxResult<()> {
        let devices = update_remote(&state, |r| {
            let d = r.devices.iter_mut().find(|d| d.id == device_id)?;
            d.cap = cap.as_str().to_string();
            Some(r.devices.clone())
        })?
        .ok_or_else(|| cfg_err("unknown device"))?;
        let paired = sh.paired(&devices).usable;
        sh.with_engine(|e| {
            e.set_devices(paired);
            e.device_cap_changed(&device_id, now_ms());
        });
        Ok(())
    })?;
    sh.changed();
    Ok(status_of(sh))
}

#[tauri::command(async)]
pub fn remote_device_remove(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    state: State<'_, AppState>,
    device_id: String,
) -> YmuxResult<RemoteStatus> {
    guard_local(&webview, &request, "remote_device_remove")?;
    let sh = &rt.shared;
    sh.devset.mutate(|| -> YmuxResult<()> {
        // The key first: an unreadable store refuses before anything changes.
        sh.secrets
            .set_checked(&link_secret_id(&device_id), "secret", "")?;
        let devices = update_remote(&state, |r| {
            r.devices.retain(|d| d.id != device_id);
            r.devices.clone()
        })?;
        let paired = sh.paired(&devices).usable;
        sh.with_engine(|e| {
            e.set_devices(paired);
            e.device_removed(&device_id, now_ms());
        });
        Ok(())
    })?;
    sh.changed();
    Ok(status_of(sh))
}

/// The page's answer to a `remote://request`. `data` is the protocol's reply
/// data for that op; it is rebuilt field by field before it leaves.
#[tauri::command(async)]
pub fn remote_reply(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    id: u64,
    data: Value,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "remote_reply")?;
    rt.shared
        .with_engine(|e| e.frontend_reply(id, &data, now_ms()));
    Ok(())
}

/// The page's current pane list (protocol `Pane` shape), pushed on change.
#[tauri::command(async)]
pub fn remote_publish_panes(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, RemoteRuntime>,
    panes: Value,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "remote_publish_panes")?;
    let sh = &rt.shared;
    if !sh.active.load(Ordering::Relaxed) {
        *sh.latest_panes.lock() = panes;
        return Ok(());
    }
    sh.with_engine(|e| e.publish_panes(&panes, now_ms()));
    *sh.latest_panes.lock() = panes;
    Ok(())
}

#[cfg(test)]
mod e2e;

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine for the server at `base`, with its origin set.
    fn engine(id: &str, key: Keypair, base: &str) -> Engine {
        let mut e = Engine::new(id, "e2e", key);
        e.set_origin(&server_origin(base).unwrap());
        e
    }

    /// Drive an engine over a real socket until it is ready or closed.
    fn run_handshake(url: &str, e: &mut Engine) -> Result<Ws, String> {
        let mut ws = connect(url)?;
        set_read_timeout(&mut ws, Duration::from_secs(5));
        e.on_connect();
        e.drain();
        for _ in 0..4 {
            let msg = ws.read().map_err(|e| format!("read: {e}"))?;
            if let Message::Text(t) = msg {
                e.on_text(t.as_str(), now_ms());
            }
            for o in e.drain() {
                match o {
                    Out::Send(f) => ws.send(Message::text(f)).map_err(|e| e.to_string())?,
                    Out::Ready => return Ok(ws),
                    Out::Close(code) => return Err(code),
                    _ => {}
                }
            }
        }
        Err("no ready".into())
    }

    /// A real handshake against a running yMux-Server (a manual check, not
    /// part of CI): `YMUX_E2E_URL=http://127.0.0.1:11020YMUX_E2E_TOKEN=<setup
    /// token> cargo test -p ymux --lib -- --ignored real_server`.
    #[test]
    #[ignore = "needs a running yMux-Server and its setup token"]
    fn real_server_handshake_registers_and_reconnects() {
        let base = std::env::var("YMUX_E2E_URL").expect("YMUX_E2E_URL");
        let token = std::env::var("YMUX_E2E_TOKEN").expect("YMUX_E2E_TOKEN");
        let url = ws_url(&base).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let key = Keypair::generate();
        let secret = key.secret_b64u();

        // First contact: unknown desktop id, the token registers the key.
        let mut e = engine(&id, key, &base);
        e.set_identity("e2e", &token);
        let mut ws = run_handshake(&url, &mut e).expect("first handshake");
        // A pairing window opens and closes without the server complaining.
        let (code, _, _) = e.pair_open(now_ms()).unwrap();
        assert!(!code.is_empty());
        e.pair_cancel();
        for o in e.drain() {
            if let Out::Send(f) = o {
                ws.send(Message::text(f)).unwrap();
            }
        }
        let _ = ws.close(None);

        // Second contact: known id, the stored key, no token.
        let mut e = engine(&id, Keypair::from_b64u(&secret).unwrap(), &base);
        e.set_identity("e2e", "");
        run_handshake(&url, &mut e).expect("reconnect without a token");

        // A different key for the same id is refused.
        let mut e = engine(&id, Keypair::generate(), &base);
        e.set_identity("e2e", "");
        assert_eq!(
            run_handshake(&url, &mut e).err().as_deref(),
            Some("key_mismatch")
        );

        // A new id with a wrong token is refused.
        let mut e = engine(
            &uuid::Uuid::new_v4().to_string(),
            Keypair::generate(),
            &base,
        );
        e.set_identity("e2e", "wrong");
        assert_eq!(
            run_handshake(&url, &mut e).err().as_deref(),
            Some("bad_token")
        );
    }
}
