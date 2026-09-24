//! Resolving and opening filesystem paths that the frontend lifted out of
//! terminal output.
//!
//! The frontend's path linkifier hands raw substrings of whatever a shell
//! printed — `cat`, `git log`, an agent's answer — and asks two things:
//! "does this exist?" and, on a click, "open it with the OS default
//! program". Both questions are answered here rather than in `commands.rs`
//! so the parts worth testing (validation, tilde expansion, the UNC policy,
//! the reveal-instead-of-run list) stay reachable from
//! `cargo test --no-default-features --lib -p ymux` on Linux CI.
//!
//! ## The threat model, because it is not the obvious one
//!
//! Terminal output is attacker-influenced. Anything the user `cat`s, any
//! repository they clone, any page an agent quotes can plant a string that
//! this module will be asked about. Two consequences shape the code:
//!
//!  1. **Probing is automatic.** Existence checks fire on hover, without a
//!     click. On Windows, statting `\\evil.example\x\y` makes the SMB client
//!     open a connection and offer NTLM credentials, which is a credential
//!     leak triggered by moving a mouse; an unreachable host also hangs the
//!     call for tens of seconds. So auto-probing a remote UNC host is
//!     refused unless it is a known-local pseudo-host. See
//!     [`network_probe_allowed`] and [`resolve_local`].
//!
//!     The check is made on the path that would actually be stat'd — the
//!     candidate *joined onto the cwd and normalised* — never on the raw
//!     candidate, because the cwd is attacker-influenced too: it arrives by
//!     OSC 7, which any printed output can emit. `src/a.rs` under a cwd of
//!     `\\evil\share` is a network path. For the same reason there is no
//!     "the pane already lives on that share" exception any more: that fact
//!     came from the same forgeable OSC 7. Symlinks and junctions are read
//!     with `symlink_metadata`/`read_link` and their targets classified
//!     before anything follows them.
//!  2. **"Open with the default program" runs executables.** `ShellExecuteW`
//!     on `evil.bat` does not open it, it executes it — and the user's
//!     mental model for clicking a link is "show me this", not "run this".
//!     Only an allowlist of document types is opened; everything else —
//!     and anything with an execute bit — is revealed in the file manager
//!     instead. See [`should_reveal`].
//!
//! Nothing here ever builds a shell command line. `opener` uses
//! `ShellExecuteW` on Windows and `Command::new("open")` on macOS, both of
//! which take the path as an opaque argument, so a filename containing `&`,
//! `^`, `%` or a quote is inert.

use std::path::{Path, PathBuf};

/// Only the app's own main webview may call a `guard_local` command.
///
/// This is not theoretical: Tauri injects `invoke` into every webview it
/// creates, capability or not, and never ACL-checks ymux's own commands, so
/// the page loaded in an `eb-*` embedded browser pane can call any of them.
/// Without this check any website the user visits could spawn a program,
/// type into a shell or read the filesystem.
pub fn caller_allowed(label: &str) -> bool {
    label == "main"
}

/// Does an IPC request's `Origin` header name ymux's own document?
///
/// The label half ([`caller_allowed`]) is **not sufficient** on its own, and
/// this is the evidence, read out of the pinned dependency sources rather
/// than assumed:
///
///  - A `PaneKind::Browser` pane is an `<iframe>` *inside* the `main`
///    webview (`frame-src http: https:` in `tauri.conf.json`), so anything
///    running in it sees `webview.label() == "main"`.
///  - wry hands every initialization script to WebView2's
///    `AddScriptToExecuteOnDocumentCreated` and drops the
///    `for_main_frame_only` flag on the floor (`wry-0.54.4`
///    `src/webview2/mod.rs:494`; the flag is documented as ignored on
///    Windows at `src/lib.rs:1007`). Tauri asks for main-frame-only
///    (`tauri-2.10.3` `src/manager/webview.rs:156`) and does not get it, so
///    an iframe is handed `window.__TAURI_INTERNALS__` *including the
///    invoke key* — which is the only pre-dispatch check Tauri performs
///    (`src/webview/mod.rs:1729`).
///
/// The origin, unlike the label, does distinguish them. Tauri's own JS
/// reaches the IPC with `fetch()` (`scripts/ipc-protocol.js`), and on that
/// path the `Origin` header is read from the real HTTP request
/// (`src/ipc/protocol.rs:491`, stored at `:549`) and forwarded to the
/// command through `tauri::ipc::Request::headers()`. `Origin` is a
/// forbidden header name, so a page cannot set it — the browser does, from
/// the frame's own origin.
///
/// Compared component-wise on purpose: `Url::origin()` returns an *opaque*
/// origin for a non-special scheme, and two opaque origins never compare
/// equal, so `a.origin() == b.origin()` would reject macOS's own
/// `tauri://localhost`.
///
/// `allowed` is built from the **configuration** ([`allowed_origins`]), not
/// from whatever the webview currently shows. That distinction is
/// load-bearing: comparing against `webview.url()` would mean that if the
/// `main` webview were ever navigated to a remote page, that page's origin
/// would trivially equal the app origin and the guard would pass. Tauri's
/// own `is_local_url` compares against the configured app URL for the same
/// reason. (Today `BrowserPane`'s iframe carries a `sandbox` without
/// `allow-top-navigation` and no navigation handler exists, so the
/// navigation is not reachable — the guard simply does not depend on that
/// staying true.)
///
/// Fails closed. A missing header, `null` (a sandboxed or `data:` frame), an
/// unparseable value or a host-only near-miss such as
/// `http://tauri.localhost.evil.com` all return `false`.
///
/// ## What this relies on: an iframe cannot reach the `postMessage` IPC
///
/// Tauri's fallback transport (`window.ipc.postMessage`, used when the
/// `fetch` to the IPC protocol fails) builds the request headers from a
/// JSON field **the page supplies** (`handle_ipc_message`, `tauri-2.10.3` `src/ipc/protocol.rs:185`), so on
/// that path `Origin` is forgeable. What keeps a `browser`-pane iframe off
/// it on Windows is WebView2 itself: wry subscribes only to
/// `ICoreWebView2::add_WebMessageReceived` (`wry-0.54.4`
/// `src/webview2/mod.rs:892`), which fires for the *top-level* document;
/// an iframe's `chrome.webview.postMessage` is delivered to
/// `ICoreWebView2Frame2::WebMessageReceived`, which needs a `FrameCreated`
/// subscription wry never makes. The iframe's only working transport is
/// `fetch`, where the browser sets `Origin`. An `eb-*` child *is* a
/// top-level document and can forge `Origin` — which is why the label check
/// ([`caller_allowed`]) is not optional. If a wry/Tauri upgrade ever starts
/// handling frame web messages, this reasoning must be revisited.
///
/// **macOS rests on a different fact.** There wry accepts `postMessage`
/// from *any* frame (`wry-0.54.4` `src/wkwebview/class/wry_web_view_delegate.rs:50`
/// reads the sending frame's URL but does not filter on it), and
/// [`request_is_local`] checks the webview's *top-level* URL, not the
/// sending frame's — so the transport alone would let a framed page through.
/// What keeps a macOS iframe out is that it never gets the invoke key:
/// WKWebView honours main-frame-only injection
/// (`src/wkwebview/mod.rs:644`, `:781`, `forMainFrameOnly`), so Tauri's IPC
/// scripts never run in a subframe and every message it could send is
/// rejected by `on_message`'s invoke-key check. Re-check this too on any
/// wry/Tauri upgrade.
///
/// ## What this cannot see: ymux's own document inside a frame
///
/// If a `browser` pane framed `http://tauri.localhost/` itself, that frame's
/// requests would carry label `main` *and* the local Origin, and nothing in
/// the request tells a subframe from the top-level document (the fetch
/// headers — `Sec-Fetch-*`, `Referer` — are the same for both). That case is
/// stopped before it can issue a request: the CSP's `frame-ancestors 'none'`
/// refuses the load, and `src/bootGuard.ts` refuses to boot when
/// `window.top !== window`.
pub fn origin_is_local<S: AsRef<str>>(origin: Option<&str>, allowed: &[S]) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    allowed.iter().any(|a| same_origin(origin, a.as_ref()))
}

/// Component-wise origin equality for one candidate.
fn same_origin(origin: &str, app_url: &str) -> bool {
    if origin.trim().eq_ignore_ascii_case("null") {
        return false;
    }
    let (Ok(got), Ok(want)) = (url::Url::parse(origin), url::Url::parse(app_url)) else {
        return false;
    };
    let (Some(got_host), Some(want_host)) = (got.host_str(), want.host_str()) else {
        // An origin with no authority (`data:`, `file:`) is never ymux's
        // document, and an app URL without one is a configuration we do not
        // ship — either way, refuse.
        return false;
    };
    got.scheme().eq_ignore_ascii_case(want.scheme())
        && got_host.eq_ignore_ascii_case(want_host)
        && got.port_or_known_default() == want.port_or_known_default()
}

/// Which of Tauri's two IPC transports delivered a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcTransport {
    /// `fetch()` to the IPC custom protocol. Headers are the real HTTP
    /// request's, so `Origin` is set by the browser and cannot be forged.
    Fetch,
    /// `window.ipc.postMessage`, Tauri's fallback once a `fetch` has failed.
    /// Headers are a JSON field the *page* supplies (`handle_ipc_message`,
    /// `tauri-2.10.3` `src/ipc/protocol.rs:185`), normally empty.
    PostMessage,
}

impl IpcTransport {
    /// Tauri's `ipc-protocol.js` always sends the invoke key as the
    /// `Tauri-Invoke-Key` header on the `fetch` path — the protocol handler
    /// refuses a request without it (`src/ipc/protocol.rs:482`) — while on
    /// the `postMessage` path the key is a body field. A page on the
    /// `postMessage` path *can* add the header to pose as `Fetch`; that only
    /// moves it onto the stricter branch of [`request_is_local`].
    pub fn from_invoke_key_header(present: bool) -> Self {
        if present {
            Self::Fetch
        } else {
            Self::PostMessage
        }
    }
}

/// Did this IPC request come from ymux's own document?
///
/// - A **present** `Origin` must be local ([`origin_is_local`]), on either
///   transport.
/// - A **missing** `Origin` is accepted only on the `postMessage` path, and
///   only when the webview's current top-level URL is ymux's own. Tauri's JS
///   switches to `postMessage` *permanently* after any failed IPC `fetch`
///   (`scripts/ipc-protocol.js`), and that path sends no `Origin`, so
///   refusing it would brick the app until restart. It is safe because only
///   a webview's top-level document reaches that path on WebView2 (see
///   "What this relies on" at [`origin_is_local`]); a framed page is on
///   `fetch`, where its real Origin is always present.
/// - `postMessage` additionally requires the current URL to be local even
///   when an Origin is present, since there the Origin is page-supplied.
///
/// The label is checked separately ([`caller_allowed`]).
pub fn request_is_local<S: AsRef<str>>(
    origin: Option<&str>,
    transport: IpcTransport,
    current_url: Option<&str>,
    allowed: &[S],
) -> bool {
    let page_is_local = || origin_is_local(current_url, allowed);
    match (origin, transport) {
        (Some(o), IpcTransport::Fetch) => origin_is_local(Some(o), allowed),
        (Some(o), IpcTransport::PostMessage) => {
            origin_is_local(Some(o), allowed) && page_is_local()
        }
        (None, IpcTransport::PostMessage) => page_is_local(),
        (None, IpcTransport::Fetch) => false,
    }
}

/// The `Origin` header of the IPC request now being served, if any.
///
/// Split out from [`guard_local`] so the header-name lookup is in one place
/// and the decision itself stays in the pure [`request_is_local`].
#[cfg(feature = "desktop")]
fn request_origin<'a>(request: &'a tauri::ipc::Request<'_>) -> Option<&'a str> {
    request.headers().get("Origin")?.to_str().ok()
}

/// The gate on every ymux command whose only legitimate caller is ymux's
/// own document — which is all of them except the two in
/// [`crate::ipc_guard::EMBEDDED_CHILD_COMMANDS`].
///
/// Called on the first line of every one of those commands, and
/// `ipc_guard::tests::every_registered_command_starts_with_a_guard` fails if
/// one is missing (CLAUDE.md rule 16). It is not
/// defence in depth — it is the *only* defence, because `src-tauri/build.rs`
/// is a bare `tauri_build::build()` with no `AppManifest`, so
/// `RuntimeAuthority::has_app_manifest()` is false and Tauri skips the ACL
/// check entirely for ymux's own commands (`tauri-2.10.3`
/// `src/webview/mod.rs:1802`). The capability files govern `core:` and
/// plugin permissions only; adding ymux's commands to one would require an
/// `AppManifest`, which would switch ACL enforcement on for every
/// command at once. See the spec's §1.5.
///
/// `cmd` only names the caller in the error message.
#[cfg(feature = "desktop")]
pub fn guard_local(
    webview: &tauri::Webview,
    request: &tauri::ipc::Request<'_>,
    cmd: &str,
) -> crate::YmuxResult<()> {
    if !caller_allowed(webview.label()) {
        return Err(crate::YmuxError::Forbidden(format!(
            "{cmd}: only ymux's own webview may call this (label {:?})",
            webview.label()
        )));
    }
    let transport =
        IpcTransport::from_invoke_key_header(request.headers().contains_key("Tauri-Invoke-Key"));
    let current_url = webview.url().ok();
    if !request_is_local(
        request_origin(request),
        transport,
        current_url.as_ref().map(url::Url::as_str),
        &allowed_origins(webview),
    ) {
        return Err(crate::YmuxError::Forbidden(format!(
            "{cmd}: only ymux's own document may call this, not embedded web content"
        )));
    }
    Ok(())
}

/// The origins ymux's own document can legitimately have, derived from the
/// running configuration rather than from the webview's current URL.
///
/// Mirrors `AppManager::tauri_protocol_url` (`tauri-2.10.3`
/// `src/manager/mod.rs:331`): Windows and Android serve the app over
/// `http(s)://tauri.localhost`, everything else over `tauri://localhost`.
/// The dev-server origin is added only in a dev build, so a release binary
/// never accepts `http://localhost:1420`.
#[cfg(feature = "desktop")]
fn allowed_origins(webview: &tauri::Webview) -> Vec<String> {
    use tauri::Manager;
    let cfg = webview.config();

    let https = cfg.app.windows.iter().any(|w| w.use_https_scheme);
    let mut out = Vec::with_capacity(2);
    if cfg!(windows) || cfg!(target_os = "android") {
        out.push(if https {
            "https://tauri.localhost".to_string()
        } else {
            "http://tauri.localhost".to_string()
        });
    } else {
        out.push("tauri://localhost".to_string());
    }

    #[cfg(dev)]
    if let Some(dev_url) = &cfg.build.dev_url {
        out.push(dev_url.to_string());
    }

    out
}

/// Longest raw candidate worth looking at. Comfortably past any real path
/// (`MAX_PATH` is 260, and even the extended limit is 32767) while keeping a
/// pathological line from turning into a long syscall.
pub const MAX_RAW_LEN: usize = 4096;

/// Most candidates probed in one request. The frontend sends at most one
/// row's worth (24), so this is a backstop against a caller that does not.
pub const MAX_BATCH: usize = 32;

/// How long a batch of existence checks may take before it is abandoned.
/// A mapped network drive that has gone away blocks `metadata` for tens of
/// seconds; a hover must not be able to wedge anything for that long.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// A resolved, existing path, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPath {
    /// Absolute, with Windows' verbatim `\\?\` prefix stripped so it is both
    /// readable in the tooltip and acceptable to `ShellExecuteW`.
    pub absolute: String,
    /// Directories open in the OS file manager rather than an application.
    pub is_dir: bool,
}

/// Why a raw candidate is not worth touching at all, or `None` if it is.
///
/// Existence is the main filter, but deliberately not the only one: a
/// control character must not reach the opener even if some filesystem
/// somewhere would accept it in a name.
pub fn reject_reason(raw: &str) -> Option<&'static str> {
    if raw.is_empty() {
        return Some("empty path");
    }
    if raw.len() > MAX_RAW_LEN {
        return Some("path too long");
    }
    if raw.chars().any(|c| c.is_control()) {
        return Some("path contains a control character");
    }
    None
}

/// How a path's *syntax* engages Windows' UNC and device namespaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UncKind {
    /// An ordinary path. Nothing special about it.
    Local,
    /// `\\?\C:\…` — the verbatim spelling of a local drive path.
    VerbatimLocal,
    /// `\\.\…`, or a `\\?\…` form that is neither a drive nor a UNC share.
    /// The Win32 device namespace: `\\.\PhysicalDrive0`, `\\.\COM1`. Never
    /// probed and never opened.
    Device,
    /// A network share. The payload is the host.
    Remote(String),
}

/// Classify `raw`'s UNC syntax, by **Win32's** rules rather than Rust's.
///
/// Rust's `Path` prefix parser is not a faithful model of what
/// `CreateFileW` does, and it is wrong in both directions (measured on
/// Windows 11 with Rust 1.97):
///
///  - `//?/UNC/host/share` parses as `UNC("?", "UNC")`, but Win32 turns it
///    into the verbatim `\\?\UNC\host\share` — a real share.
///  - `\??\UNC\host\share` is *not absolute* to Rust (`RootDir` first), yet
///    `std::fs::metadata` on it reaches SMB: `\??\` is the NT object
///    namespace, and Win32 hands a path starting with it to the kernel
///    untouched.
///
/// So the rules here are:
///
///  - Two leading separators start UNC or device syntax. On Windows any mix
///    of `\` and `/` counts (`\\`, `//`, `\/`, `/\` all reach the same
///    share); elsewhere only `\\`, because on macOS `//Users/me` is an
///    ordinary path, and the hazard (the local SMB client authenticating on
///    a `stat`) belongs to the machine doing the syscall, not the spelling.
///  - A leading `\??\` (and on Windows `/??/` and mixes) is the NT
///    namespace, read like `\\?\`.
///  - `\\?\` and `\??\` followed by `UNC` — matched ASCII case-insensitively,
///    as the object manager does — are shares. A drive letter is local. Any
///    other verbatim or `\\.\` form is the device namespace, which also
///    covers `\\.\UNC\host\share` and `\\?\GLOBALROOT\Device\Mup\host\…`,
///    both of which reach SMB and so must never be classified as local.
pub fn classify_unc(raw: &str) -> UncKind {
    let is_sep = |c: char| c == '\\' || (cfg!(windows) && c == '/');
    let mut chars = raw.chars();
    let lead: [Option<char>; 4] = [chars.next(), chars.next(), chars.next(), chars.next()];

    // `\??\` — the NT object namespace, passed through to the kernel.
    if let [Some(a), Some('?'), Some('?'), Some(d)] = lead {
        if is_sep(a) && is_sep(d) {
            return classify_verbatim_tail(&raw[4..]);
        }
    }

    let two_seps = match lead {
        [Some('\\'), Some('\\'), ..] => true,
        [Some(a), Some(b), ..] => is_sep(a) && is_sep(b),
        _ => false,
    };
    if !two_seps {
        return UncKind::Local;
    }
    let rest = &raw[2..];
    let head = rest.split(['\\', '/']).next().unwrap_or("");
    match head {
        // `\\.\` — the Win32 device namespace, `\\.\UNC\host\share` included.
        "." => UncKind::Device,
        "?" => classify_verbatim_tail(&rest[1..]),
        "" => UncKind::Device,
        host => UncKind::Remote(host.to_string()),
    }
}

/// What follows `\\?\` or `\??\`: a share, a drive, or a device.
fn classify_verbatim_tail(tail: &str) -> UncKind {
    let tail = tail.trim_start_matches(['\\', '/']);
    // `UNC\server\share` is a UNC path in verbatim clothing. Case-insensitive:
    // `\\?\unc\host\share` reaches the share exactly as `UNC` does.
    let unc = tail
        .get(..3)
        .filter(|p| p.eq_ignore_ascii_case("UNC"))
        .and_then(|_| tail.get(3..))
        .filter(|r| r.is_empty() || r.starts_with(['\\', '/']));
    if let Some(unc) = unc {
        return match unc
            .trim_start_matches(['\\', '/'])
            .split(['\\', '/'])
            .next()
            .unwrap_or("")
        {
            "" => UncKind::Device,
            host => UncKind::Remote(host.to_string()),
        };
    }
    // `\\?\C:\…` — a drive letter, colon, then a separator or end.
    let mut chars = tail.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(d), Some(':'), sep)
            if d.is_ascii_alphabetic() && matches!(sep, None | Some('\\') | Some('/')) =>
        {
            UncKind::VerbatimLocal
        }
        _ => UncKind::Device,
    }
}

/// Is `name` one of the DOS device names Win32 maps to a device in any
/// directory (`C:\x\COM1`, `C:\x\nul.txt`)? Such a path is the device
/// namespace in disguise, so it is never probed or opened.
fn is_dos_device_name(name: &str) -> bool {
    // Win32 ignores everything from the first `.` or `:`, and trailing spaces.
    let stem = name.split(['.', ':']).next().unwrap_or("").trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    // COM1-9 and LPT1-9, plus the superscript digits Win32 also accepts.
    let Some(n) = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
    else {
        return false;
    };
    matches!(
        n,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

/// Hosts whose "network" paths never leave the machine, so probing them
/// cannot leak a credential or block on an unreachable server.
const LOCAL_UNC_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    // WSL's two spellings of the distro filesystem.
    "wsl$",
    "wsl.localhost",
];

fn same_host(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// May `path` be stat'd automatically, on hover, with no click?
///
/// `path` must be the path that will actually be touched — joined onto the
/// cwd and normalised ([`resolve_local`] does that) — never a raw
/// candidate: a relative `src/a.rs` is only as local as the cwd it lands in.
///
/// There is deliberately no exception for "the host the pane's cwd is on".
/// The cwd comes from OSC 7, which any printed output can forge, so that
/// exception let a planted escape sequence whitelist an attacker's host. A
/// user who really works on a share simply gets no links there — the safe
/// way to fail.
///
/// The pseudo-hosts in [`LOCAL_UNC_HOSTS`] stay allowed for an explicit
/// absolute candidate such as `\\wsl$\Ubuntu\home\me\x`: those never leave
/// the machine. (A *cwd* on one is still refused, by `pty::osc7`.)
pub fn network_probe_allowed(path: &str) -> Result<(), String> {
    if cfg!(windows) {
        let last = path.rsplit(['\\', '/']).next().unwrap_or("");
        if is_dos_device_name(last) {
            return Err("DOS device names are not resolved".into());
        }
    }
    match classify_unc(path) {
        UncKind::Local | UncKind::VerbatimLocal => Ok(()),
        UncKind::Device => Err("device-namespace paths are not resolved".into()),
        UncKind::Remote(host) => {
            if LOCAL_UNC_HOSTS.iter().any(|h| same_host(h, &host)) {
                Ok(())
            } else {
                Err(format!(
                    "network path on host {host:?} is not resolved automatically"
                ))
            }
        }
    }
}

/// Is `path` acceptable as a pane's working directory?
///
/// Stricter than [`network_probe_allowed`]: a cwd is a local directory, so
/// every UNC, verbatim-UNC, NT-namespace and device form is refused,
/// pseudo-local hosts included. Used by `pty::osc7` so a planted OSC 7 can
/// never make `\\evil\share` the base that relative paths are resolved on.
pub fn cwd_is_local(path: &str) -> bool {
    matches!(classify_unc(path), UncKind::Local | UncKind::VerbatimLocal)
}

/// Most symlink/junction hops [`walk_links`] follows before giving up.
/// Matches Linux's `MAXSYMLINKS` order of magnitude; a loop hits it fast.
pub const MAX_LINK_HOPS: usize = 32;

/// One step of a path still to be walked.
enum Part {
    /// Prefix and/or root: replaces everything walked so far.
    Root(PathBuf),
    Parent,
    Name(std::ffi::OsString),
}

fn parts(p: &Path) -> Vec<Part> {
    use std::path::Component;
    let mut root = PathBuf::new();
    let mut has_root = false;
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => {
                root.push(c.as_os_str());
                has_root = true;
            }
            Component::CurDir => {}
            Component::ParentDir => out.push(Part::Parent),
            Component::Normal(n) => out.push(Part::Name(n.to_owned())),
        }
    }
    if has_root {
        out.insert(0, Part::Root(root));
    }
    out
}

/// Fold `.` and `..` lexically, the way Win32 does before any syscall.
///
/// Windows only: there `C:\link\..\x` *is* `C:\x` whatever `link` points at,
/// so this is exactly the path the OS will open. On POSIX `..` after a
/// symlink means the link target's parent, so the path is left for
/// [`walk_links`] to resolve in order.
fn normalise(p: &Path) -> PathBuf {
    if !cfg!(windows) {
        return p.to_path_buf();
    }
    let mut out = PathBuf::new();
    for part in parts(p) {
        match part {
            Part::Root(r) => out = r,
            Part::Parent => {
                out.pop();
            }
            Part::Name(n) => out.push(n),
        }
    }
    out
}

/// Resolve every symlink and junction in `path` **without following any
/// of them blindly**: each link is detected with `is_link` (a
/// `symlink_metadata` in production), its target read with `read_link` and
/// passed through [`network_probe_allowed`] before the walk continues into
/// it. A link — or a chain of links — that leads to a share is refused
/// before anything ever opens the share.
///
/// Returns the fully resolved path, which contains no links (modulo a race
/// with whoever is editing the tree) and so can be stat'd safely. A
/// relative target resolves against the link's own directory.
///
/// The filesystem is injected so the chain logic can be tested without the
/// privilege Windows demands for creating a symlink.
pub fn walk_links<L, R>(path: &Path, is_link: L, read_link: R) -> Result<PathBuf, String>
where
    L: Fn(&Path) -> std::io::Result<bool>,
    R: Fn(&Path) -> std::io::Result<PathBuf>,
{
    let mut todo = parts(path);
    todo.reverse();
    let mut out = PathBuf::new();
    let mut hops = 0usize;
    while let Some(part) = todo.pop() {
        match part {
            Part::Root(r) => out = r,
            Part::Parent => {
                out.pop();
            }
            Part::Name(n) => {
                let next = out.join(&n);
                if !is_link(&next).map_err(|e| e.to_string())? {
                    out = next;
                    continue;
                }
                hops += 1;
                if hops > MAX_LINK_HOPS {
                    return Err("too many levels of symbolic links".into());
                }
                let target = read_link(&next).map_err(|e| e.to_string())?;
                let spelled = target.to_string_lossy();
                network_probe_allowed(&spelled)
                    .map_err(|why| format!("{} links elsewhere: {why}", next.display()))?;
                let mut more = parts(&target);
                more.reverse();
                todo.extend(more);
            }
        }
    }
    Ok(out)
}

/// The real-filesystem [`walk_links`] probes: `symlink_metadata`, which
/// never follows, and `read_link`. On Windows `FileType::is_symlink` is
/// true for junctions as well as symlinks (both are name-surrogate reparse
/// points), which a test pins.
fn is_link_on_disk(p: &Path) -> std::io::Result<bool> {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink())
}

/// The one gate every automatic filesystem touch goes through.
///
/// `path` is the candidate already joined onto the cwd ([`expand`]). It is
/// normalised, required to be absolute (a relative result would be resolved
/// against *ymux's own* working directory, which is meaningless here),
/// classified, and its links walked and classified one by one. The result
/// contains no links and is safe to `symlink_metadata`/open.
pub fn resolve_local(path: &Path) -> Result<PathBuf, String> {
    let norm = normalise(path);
    let spelled = norm.to_string_lossy();
    network_probe_allowed(&spelled)?;
    // A share is absolute on Windows only; on other targets a `\\` path is
    // a relative name and is refused here like any other.
    if !norm.is_absolute() {
        return Err("only absolute paths are resolved".into());
    }
    let walked = walk_links(&norm, is_link_on_disk, |p| std::fs::read_link(p))?;
    network_probe_allowed(&walked.to_string_lossy())?;
    Ok(walked)
}

/// Turn a raw candidate into the absolute path it names, or `None` when it
/// cannot be placed — a relative candidate with no cwd, or a `~` path on a
/// system with no home directory.
///
/// A rooted-but-driveless path on Windows (`/usr/lib`, `\srv\x`) is *not*
/// absolute to Rust, so it takes the `cwd` branch — where `Path::join`
/// keeps the cwd's drive and replaces the rest, giving `D:\usr\lib`. That
/// is Windows' own drive-relative rule, so it is the right answer, but it
/// means a POSIX path pasted into a Windows pane can resolve to a real
/// local file with a different meaning. Harmless in practice: the link only
/// appears when that file exists, the tooltip shows the absolute path that
/// was resolved, and a click opens that same path.
pub fn expand(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let tilde = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .map(Some)
        .or_else(|| if raw == "~" { Some(None) } else { None });

    let candidate = match tilde {
        Some(rest) => {
            let home = home?;
            match rest {
                Some(r) => home.join(r),
                None => home.to_path_buf(),
            }
        }
        None => PathBuf::from(raw),
    };

    if candidate.is_absolute() {
        return Some(candidate);
    }
    Some(cwd?.join(candidate))
}

/// Drop Windows' verbatim prefix from a canonicalised path.
///
/// `std::fs::canonicalize` returns `\\?\C:\x` and `\\?\UNC\host\share\x`.
/// Neither belongs in a tooltip, and `ShellExecuteW` does not reliably
/// accept the verbatim form, so both are rewritten to their plain spelling.
/// A no-op on every other platform and on paths that do not carry it.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Only the drive form is safe to unwrap; `\\?\Volume{…}` is not.
        let mut chars = rest.chars();
        if let (Some(d), Some(':')) = (chars.next(), chars.next()) {
            if d.is_ascii_alphabetic() {
                return PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// File types a click may hand to the OS default program. **Everything
/// else is revealed** in the file manager instead of opened.
///
/// An allowlist because the previous denylist could not be complete: every
/// interpreter installer registers its own "open = run" association
/// (`.py`, `.pyw`, `.sh` under Git for Windows, `.rb`, `.pl`), Windows keeps
/// adding executable document types (`.settingcontent-ms`, `.appinstaller`,
/// `.application`, `.library-ms`, `.search-ms`, `.xll`, `.wsc`, `.sct`,
/// `.chm`), and disk images auto-mount (`.iso`, `.vhd`, `.vhdx`). A type
/// missing from this list costs one extra click in the file manager; a type
/// missing from a denylist cost code execution.
///
/// Every entry is a *document* for which no mainstream default handler
/// executes content on open. Deliberately absent, although they look like
/// "source files":
///
///  - `js` (Windows Script Host runs it), `jsx` (Adobe ExtendScript),
///    `py`/`pyw`/`sh`/`rb`/`pl`/`php`/`ps1`/`lua`/`tcl` (interpreters
///    register "open" as "run"), `jar`;
///  - `sln`/`csproj`/`vcxproj` and friends: Visual Studio runs MSBuild
///    targets when it loads a project;
///  - `xml` (an `mso-application` processing instruction routes it to Office
///    as a macro-capable document), `csv`/`tsv` (Excel formulas and DDE),
///    `rtf` and the legacy/macro-enabled Office formats (`doc`, `xls`, `ppt`,
///    `docm`, `xlsm`, `pptm`, …).
const OPENABLE_EXTENSIONS: &[&str] = &[
    // Plain text, markup and data. Opened in an editor or viewer.
    "txt", "text", "log", "md", "markdown", "rst", "adoc", "json", "jsonc", "json5", "jsonl",
    "yaml", "yml", "toml", "ini", "cfg", "conf", "lock", "diff", "patch", "sql", //
    // Source code in languages with no "double-click runs it" association.
    "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "java", "kt", "go", "swift", "ts", "tsx",
    "css", "scss", "sass", "less", "vue", "svelte", "proto", "graphql", "zig", //
    // Images.
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "tif", "tiff", "avif", "heic", //
    // PDF.
    "pdf", //
    // Office documents that cannot carry macros (OOXML without the `m`,
    // OpenDocument).
    "docx", "xlsx", "pptx", "odt", "ods", "odp", //
    // HTML: opens a browser, which sandboxes the page's script. The same
    // exposure as clicking a URL link in the same pane.
    "html", "htm",
];

/// Does `meta` carry an execute bit? macOS `open` runs an extensionless
/// executable in Terminal, and a `.txt` with `+x` is suspicious enough to
/// be shown rather than opened. Always `false` on Windows, which has no
/// such bit — there the extension is the whole story.
pub fn exec_bit(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        !meta.is_dir() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

/// Should this path be revealed in the file manager instead of opened?
///
/// `path` must be the **resolved** path (links followed, see
/// [`resolve_local`]), so the name judged is the name of what will open.
///
/// - A name containing `:` is revealed: past the drive it can only be an
///   NTFS alternate data stream, and `evil.exe:x.txt` would otherwise pass
///   as a `.txt`.
/// - A directory opens in the file manager — that *is* "show it" — unless its
///   name has an extension. That catches every macOS bundle (`Foo.app`,
///   `x.pkg`, `y.workflow`, `z.framework`: `open Foo.app` launches it) and
///   Windows' `folder.{CLSID}` shell-namespace junctions. A plain directory
///   named `v1.2` being revealed rather than opened is harmless.
/// - A file with an execute bit (`exec_bit`) is revealed.
/// - Otherwise a file opens only if its extension, lowercased, is **exactly**
///   an entry in [`OPENABLE_EXTENSIONS`]. So `evil.bat.` (trailing dot,
///   which Win32 strips), `evil.bat ` and an extensionless file are all
///   revealed.
pub fn should_reveal(path: &Path, is_dir: bool, executable: bool) -> bool {
    let Some(name) = path.file_name() else {
        // A root (`C:\`, `/`): a directory with no name to judge.
        return !is_dir;
    };
    let name = name.to_string_lossy();
    if name.contains(':') {
        return true;
    }
    let ext = Path::new(name.as_ref())
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if is_dir {
        return ext.is_some();
    }
    if executable {
        return true;
    }
    match ext {
        Some(e) => !OPENABLE_EXTENSIONS.contains(&e.as_str()),
        None => true,
    }
}

/// Final gate before handing a path to the OS opener.
///
/// The path reaching here came from a probe that already vetted it, but it
/// arrives back over IPC, so it is checked again from scratch. Absoluteness
/// is load-bearing beyond tidiness: macOS's `open` takes flags, and a
/// relative candidate beginning with `-` would become one.
pub fn validate_open(raw: &str) -> Result<(), String> {
    if let Some(why) = reject_reason(raw) {
        return Err(why.to_string());
    }
    if matches!(classify_unc(raw), UncKind::Device) {
        return Err("device-namespace paths are not opened".into());
    }
    let path = Path::new(raw);
    // A UNC path is absolute on Windows but not when this check compiles for
    // a Unix target, so accept it explicitly rather than by platform.
    let unc = matches!(
        classify_unc(raw),
        UncKind::Remote(_) | UncKind::VerbatimLocal
    );
    if !path.is_absolute() && !unc {
        return Err("only absolute paths are opened".into());
    }
    Ok(())
}

/// Resolve one raw candidate against `cwd`, or `None` if it is not a path
/// worth linking.
///
/// Order is the security property: the candidate is joined onto the cwd
/// and the *joined* path is classified and link-walked ([`resolve_local`])
/// before anything stats it. Only the link-free result is ever passed to
/// `symlink_metadata` and `canonicalize`, and the canonical form is
/// classified once more as a backstop.
pub fn probe_one(raw: &str, cwd: Option<&str>) -> Option<ResolvedPath> {
    if reject_reason(raw).is_some() {
        return None;
    }
    let expanded = expand(raw, cwd.map(Path::new), dirs::home_dir().as_deref())?;
    let resolved = resolve_local(&expanded).ok()?;
    let meta = std::fs::symlink_metadata(&resolved).ok()?;
    let absolute = std::fs::canonicalize(&resolved)
        .map(|p| strip_verbatim(&p))
        .unwrap_or(resolved);
    let absolute = absolute.to_string_lossy().into_owned();
    network_probe_allowed(&absolute).ok()?;
    Some(ResolvedPath {
        absolute,
        is_dir: meta.is_dir(),
    })
}

/// Resolve a batch, giving up on the whole batch after [`PROBE_TIMEOUT`].
///
/// The timeout runs on a detached worker rather than around each path: a
/// dead network drive blocks in `metadata` with no way to cancel it, so the
/// only thing that can be bounded is how long the *caller* waits. The output
/// always has exactly `raws.len()` entries so the frontend's index mapping
/// holds whatever happened.
pub fn probe_batch(raws: Vec<String>, cwd: Option<String>) -> Vec<Option<ResolvedPath>> {
    let want = raws.len();
    let batch: Vec<String> = raws.into_iter().take(MAX_BATCH).collect();
    let taken = batch.len();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let probed: Vec<Option<ResolvedPath>> = batch
            .iter()
            .map(|raw| probe_one(raw, cwd.as_deref()))
            .collect();
        // The receiver is gone on timeout; dropping the result is the point.
        let _ = tx.send(probed);
    });
    let mut out = rx
        .recv_timeout(PROBE_TIMEOUT)
        .unwrap_or_else(|_| vec![None; taken]);
    out.resize(want, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_webview_may_ask() {
        assert!(caller_allowed("main"));
        // Tauri injects `invoke` into embedded browser panes, so
        // a page loaded in one can reach `invoke`.
        assert!(!caller_allowed("eb-1234"));
        assert!(!caller_allowed("eb-main"));
        assert!(!caller_allowed(""));
        assert!(!caller_allowed("Main"));
    }

    /// The app origins ymux actually runs under: `tauri://localhost` on
    /// macOS, `http://tauri.localhost` in a Windows release build, and the
    /// Vite dev server under `pnpm tauri dev`.
    const APP_URLS: &[&str] = &[
        "tauri://localhost/",
        "http://tauri.localhost/",
        "http://localhost:1420/",
    ];

    #[test]
    fn origin_accepts_ymuxs_own_document() {
        assert!(origin_is_local(Some("tauri://localhost"), &[APP_URLS[0]]));
        assert!(origin_is_local(
            Some("http://tauri.localhost"),
            &[APP_URLS[1]]
        ));
        assert!(origin_is_local(
            Some("http://localhost:1420"),
            &[APP_URLS[2]]
        ));
        // A scheme is case-insensitive per RFC 3986, and so is a host.
        assert!(origin_is_local(Some("TAURI://LocalHost"), &[APP_URLS[0]]));
        // A dev build allows the app origin *and* the dev server; any one
        // match is enough.
        assert!(origin_is_local(
            Some("http://localhost:1420"),
            &[APP_URLS[1], APP_URLS[2]]
        ));
    }

    /// The list is built from configuration, never from the page currently
    /// loaded. If it were the current URL, a `main` webview that had been
    /// navigated to a remote page would hand that page an origin equal to
    /// the app's and the guard would pass.
    #[test]
    fn a_remote_page_is_refused_even_if_it_is_what_main_is_showing() {
        // "main is showing https://evil.example" is simply not expressible:
        // the allow-list never contains a remote origin.
        let allowed = [APP_URLS[1]];
        assert!(!origin_is_local(Some("https://evil.example"), &allowed));
        // An empty allow-list refuses everything rather than allowing it.
        let none: [&str; 0] = [];
        assert!(!origin_is_local(Some("http://tauri.localhost"), &none));
    }

    /// The case the label check cannot see: a page in a `browser` pane is an
    /// iframe inside the `main` webview, so only its origin gives it away.
    #[test]
    fn origin_rejects_remote_web_content() {
        for app in APP_URLS {
            assert!(!origin_is_local(Some("https://evil.example"), &[*app]));
            assert!(!origin_is_local(Some("http://evil.example"), &[*app]));
        }
        // Not even when every app origin is on the list at once.
        assert!(!origin_is_local(Some("https://evil.example"), APP_URLS));
    }

    /// Fail-closed cases. A missing header is the important one: Tauri's
    /// `postMessage` fallback carries no real headers, and accepting it
    /// would also accept a forged `Origin` on that path.
    /// Tauri's JS switches to `postMessage` for good after one failed IPC
    /// `fetch`, and that path carries no `Origin`. The top-level ymux
    /// document must keep working then, or one hiccup bricks the app.
    #[test]
    fn post_message_without_origin_is_accepted_from_ymuxs_own_page() {
        let app = [APP_URLS[1]];
        assert!(request_is_local(
            None,
            IpcTransport::PostMessage,
            Some("http://tauri.localhost/index.html"),
            &app
        ));
        assert!(request_is_local(
            None,
            IpcTransport::PostMessage,
            Some("tauri://localhost/"),
            &[APP_URLS[0]]
        ));
    }

    #[test]
    fn post_message_without_origin_needs_a_local_current_url() {
        let app = [APP_URLS[1]];
        for url in [
            None,
            Some("https://evil.example/"),
            Some("http://tauri.localhost.evil.example/"),
            Some("about:blank"),
            Some("not a url"),
        ] {
            assert!(
                !request_is_local(None, IpcTransport::PostMessage, url, &app),
                "{url:?}"
            );
        }
    }

    /// A missing Origin on the `fetch` path is impossible from a browser (a
    /// POST always carries one), so it stays a refusal.
    #[test]
    fn fetch_without_origin_is_still_refused() {
        assert!(!request_is_local(
            None,
            IpcTransport::Fetch,
            Some("http://tauri.localhost/"),
            &[APP_URLS[1]]
        ));
    }

    /// A present Origin is judged on its own, on either path: a non-local one
    /// is refused even when the webview's current URL is ymux's.
    #[test]
    fn a_present_non_local_origin_is_refused_on_either_path() {
        let app = [APP_URLS[1]];
        for transport in [IpcTransport::Fetch, IpcTransport::PostMessage] {
            for origin in ["https://evil.example", "null", ""] {
                assert!(
                    !request_is_local(
                        Some(origin),
                        transport,
                        Some("http://tauri.localhost/"),
                        &app
                    ),
                    "{origin:?} via {transport:?}"
                );
            }
        }
        assert!(request_is_local(
            Some("http://tauri.localhost"),
            IpcTransport::Fetch,
            Some("http://tauri.localhost/"),
            &app
        ));
        // postMessage also requires the current page to be ymux's, even
        // with a local-looking (and there forgeable) Origin.
        assert!(!request_is_local(
            Some("http://tauri.localhost"),
            IpcTransport::PostMessage,
            Some("https://evil.example/"),
            &app
        ));
    }

    #[test]
    fn transport_is_read_from_the_invoke_key_header() {
        assert_eq!(
            IpcTransport::from_invoke_key_header(true),
            IpcTransport::Fetch
        );
        assert_eq!(
            IpcTransport::from_invoke_key_header(false),
            IpcTransport::PostMessage
        );
    }

    #[test]
    fn origin_fails_closed() {
        let app = [APP_URLS[1]];
        assert!(!origin_is_local(None, &app));
        // A sandboxed iframe or a `data:`/`blob:` document.
        assert!(!origin_is_local(Some("null"), &app));
        assert!(!origin_is_local(Some(" NULL "), &app));
        assert!(!origin_is_local(Some(""), &app));
        assert!(!origin_is_local(Some("not a url"), &app));
        // No authority at all.
        assert!(!origin_is_local(Some("data:text/html,x"), &app));
        // An unparseable entry on the allow-list must not degrade into
        // "allow".
        assert!(!origin_is_local(
            Some("http://tauri.localhost"),
            &["nonsense"]
        ));
    }

    /// The near-misses a naive `starts_with` or `contains` would wave
    /// through.
    #[test]
    fn origin_rejects_other_local_looking_origins() {
        let app = ["http://tauri.localhost/"];
        // Tauri's own IPC endpoint and asset protocol are not ymux's
        // document; a page that can name them has not proved anything.
        assert!(!origin_is_local(Some("http://ipc.localhost"), &app));
        assert!(!origin_is_local(Some("http://asset.localhost"), &app));
        assert!(!origin_is_local(Some("http://localhost"), &app));
        assert!(!origin_is_local(Some("http://127.0.0.1"), &app));
        // Userinfo trickery: the host here is `evil.example`.
        assert!(!origin_is_local(
            Some("http://tauri.localhost@evil.example"),
            &app
        ));
        // A non-default port on the right host is a different origin.
        assert!(!origin_is_local(Some("http://tauri.localhost:8080"), &app));
        // macOS: another custom scheme on the same host is not the app.
        assert!(!origin_is_local(
            Some("ipc://localhost"),
            &["tauri://localhost/"]
        ));
        assert!(!origin_is_local(
            Some("https://localhost"),
            &["tauri://localhost/"]
        ));
    }

    /// Every label shape an embedded browser can have is refused, whatever
    /// pane id it carries.
    #[test]
    fn embedded_browser_labels_never_pass_the_label_check() {
        for label in [
            "eb-0f8fad5b-d9cb-469f-a165-70867728950e",
            "browser-0f8fad5b-d9cb-469f-a165-70867728950e",
            "main ",
            " main",
            "main\0",
        ] {
            assert!(!caller_allowed(label), "{label:?}");
        }
    }

    #[test]
    fn origin_rejects_host_and_port_near_misses() {
        let app = ["http://tauri.localhost/"];
        assert!(!origin_is_local(
            Some("http://tauri.localhost.evil.example"),
            &app
        ));
        assert!(!origin_is_local(Some("http://eviltauri.localhost"), &app));
        // Scheme must match: an https page is not the app.
        assert!(!origin_is_local(Some("https://tauri.localhost"), &app));

        // Port must match, and the default must not be confused with a
        // different explicit one.
        let dev = ["http://localhost:1420/"];
        assert!(!origin_is_local(Some("http://localhost:1421"), &dev));
        assert!(!origin_is_local(Some("http://localhost"), &dev));
        // ...but an explicit default port is the same origin.
        assert!(origin_is_local(
            Some("http://localhost:80"),
            &["http://localhost/"]
        ));
    }

    #[test]
    fn reject_reason_flags_control_characters() {
        assert!(reject_reason("src/main.ts").is_none());
        assert!(reject_reason("").is_some());
        assert!(reject_reason("a/\u{7}b").is_some());
        assert!(reject_reason("a/\nb").is_some());
        assert!(reject_reason("a/\u{1b}[0m").is_some());
        assert!(reject_reason(&"a/".repeat(MAX_RAW_LEN)).is_some());
    }

    #[test]
    fn reject_reason_allows_awkward_but_legal_names() {
        // All legal in a Windows or POSIX filename; none may be special to us
        // because nothing is ever handed to a shell.
        assert!(reject_reason(r"C:\a & b\c^d%e.txt").is_none());
        assert!(reject_reason("/srv/한글/문서.txt").is_none());
        assert!(reject_reason(r"/srv/it's a file").is_none());
    }

    #[test]
    fn classify_unc_reads_the_host() {
        assert_eq!(classify_unc(r"C:\Users\me"), UncKind::Local);
        assert_eq!(classify_unc("/usr/lib"), UncKind::Local);
        assert_eq!(classify_unc("src/main.ts"), UncKind::Local);
        assert_eq!(
            classify_unc(r"\\server\share\x"),
            UncKind::Remote("server".into())
        );
        assert_eq!(
            classify_unc(r"\\wsl$\Ubuntu\home\me"),
            UncKind::Remote("wsl$".into())
        );
    }

    #[test]
    fn classify_unc_rejects_the_device_namespace() {
        assert_eq!(classify_unc(r"\\.\PhysicalDrive0"), UncKind::Device);
        assert_eq!(classify_unc(r"\\.\COM1"), UncKind::Device);
        assert_eq!(classify_unc(r"\\"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\Volume{1234}\x"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\GLOBALROOT\Device\x"), UncKind::Device);
    }

    #[test]
    fn classify_unc_unwraps_the_verbatim_forms() {
        assert_eq!(classify_unc(r"\\?\C:\Users\me"), UncKind::VerbatimLocal);
        assert_eq!(classify_unc(r"\\?\c:"), UncKind::VerbatimLocal);
        assert_eq!(
            classify_unc(r"\\?\UNC\server\share\x"),
            UncKind::Remote("server".into())
        );
        // `\\?\UNCfoo` is not the UNC form; it is a device name.
        assert_eq!(classify_unc(r"\\?\UNCfoo\x"), UncKind::Device);
    }

    #[test]
    fn forward_slash_unc_is_windows_syntax_only() {
        // Win32 normalises `//host/share` into a UNC path; POSIX does not,
        // and `//Users/me` on macOS must stay an ordinary path.
        let kind = classify_unc("//server/share/x");
        if cfg!(windows) {
            assert_eq!(kind, UncKind::Remote("server".into()));
        } else {
            assert_eq!(kind, UncKind::Local);
        }
    }

    #[test]
    fn remote_hosts_are_not_probed_on_hover() {
        // Statting this on Windows opens SMB and offers NTLM credentials —
        // a credential leak triggered by a mouse move over terminal output.
        assert!(network_probe_allowed(r"\\evil.example\x\y").is_err());
        assert!(network_probe_allowed(r"\\10.0.0.9\share\x").is_err());
        // WebDAV-over-SSL spelling: still a remote host.
        assert!(network_probe_allowed(r"\\evil@SSL@443\share\a").is_err());
    }

    #[test]
    fn local_pseudo_hosts_are_probed() {
        assert!(network_probe_allowed(r"\\wsl$\Ubuntu\home\me").is_ok());
        assert!(network_probe_allowed(r"\\WSL.localhost\Ubuntu\etc").is_ok());
        assert!(network_probe_allowed(r"\\localhost\c$\Windows").is_ok());
    }

    /// The exception used to be "paths on the host the pane's cwd is on are
    /// fine". The cwd comes from OSC 7, which any printed output can forge,
    /// so the exception let a planted escape sequence whitelist an attacker's
    /// host. It is gone: the policy no longer even takes a cwd.
    #[test]
    fn the_panes_own_share_is_no_longer_trusted() {
        assert!(network_probe_allowed(r"\\nas\projects\a.txt").is_err());
        let cwd = Path::new(r"\\nas\projects");
        let joined = expand(r"a.txt", Some(cwd), None).unwrap();
        assert!(resolve_local(&joined).is_err());
    }

    #[test]
    fn ordinary_paths_are_always_probed() {
        assert!(network_probe_allowed(r"C:\repo\src\main.rs").is_ok());
        assert!(network_probe_allowed("/usr/lib/x").is_ok());
        assert!(network_probe_allowed("src/main.ts").is_ok());
    }

    /// HIGH 1 of the review. The raw candidate `src/a.rs` is as local as it
    /// gets — the danger is the cwd it is joined onto, which a planted OSC 7
    /// can set to a share. The classification has to see the joined path.
    #[test]
    fn a_relative_candidate_under_a_network_cwd_is_refused() {
        for cwd in [
            r"\\evil\share",
            r"\\?\UNC\evil\share",
            r"\??\UNC\evil\share",
            r"\\.\UNC\evil\share",
        ] {
            let joined = expand("src/a.rs", Some(Path::new(cwd)), None).unwrap();
            let err = resolve_local(&joined).expect_err(cwd);
            assert!(
                err.contains("network") || err.contains("device"),
                "{cwd}: {err}"
            );
            // And the probe as a whole says "not a link" without touching it.
            assert!(probe_one("src/a.rs", Some(cwd)).is_none(), "{cwd}");
        }
        if cfg!(windows) {
            for cwd in [
                "//evil/share",
                r"\/evil\share",
                r"/\evil\share",
                "//?/UNC/evil/share",
            ] {
                let joined = expand("src/a.rs", Some(Path::new(cwd)), None).unwrap();
                assert!(resolve_local(&joined).is_err(), "{cwd}");
            }
        }
    }

    /// Every spelling of "a share" and "a device" found for this fix,
    /// classified by what Win32 does with it — not by what Rust's `Path`
    /// parser says. Measured on Windows 11 / Rust 1.97:
    ///
    /// | spelling               | Rust `Path` says              | Win32 does                  |
    /// |------------------------|-------------------------------|-----------------------------|
    /// | `\\h\s`                | `UNC(h, s)`                   | share                       |
    /// | `//h/s`, `\/h\s`, `/\h\s` | `UNC(h, s)`                | share                       |
    /// | `\\?\UNC\h\s`          | `VerbatimUNC(h, s)`           | share                       |
    /// | `\\?\unc\h\s`          | `Verbatim("unc")`             | share (case-insensitive)    |
    /// | `//?/UNC/h/s`, `\\?/UNC/h/s` | `UNC("?", "UNC")`       | `\\?\UNC\h\s` — share       |
    /// | `\\.\UNC\h\s`, `//./UNC/h/s` | `DeviceNS("UNC")`       | share via the device path   |
    /// | `\??\UNC\h\s`          | `RootDir` — *not absolute*    | NT path, share (`metadata` reached SMB) |
    /// | `\\?\GLOBALROOT\Device\Mup\h\s` | `Verbatim("GLOBALROOT")` | share via the MUP device |
    /// | `\\.\PhysicalDrive0`   | `DeviceNS`                    | raw disk                    |
    #[test]
    fn every_network_and_device_spelling_is_refused() {
        let remote = [
            r"\\evil\share\a",
            r"\\?\UNC\evil\share\a",
            r"\\?\unc\evil\share\a",
            r"\\?\Unc/evil/share/a",
            r"\??\UNC\evil\share\a",
            r"\??\unc\evil\share\a",
        ];
        for s in remote {
            assert_eq!(classify_unc(s), UncKind::Remote("evil".into()), "{s}");
            assert!(network_probe_allowed(s).is_err(), "{s}");
            assert!(!cwd_is_local(s), "{s}");
        }
        let device = [
            r"\\.\UNC\evil\share\a",
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\Mup\evil\share",
            r"\??\GLOBALROOT\Device\Mup\evil\share",
            r"\\?\Volume{1234}\x",
            r"\\?\UNC\",
            r"\\",
            r"\\\evil\share",
        ];
        for s in device {
            assert_eq!(classify_unc(s), UncKind::Device, "{s}");
            assert!(network_probe_allowed(s).is_err(), "{s}");
            assert!(!cwd_is_local(s), "{s}");
        }
        // Forward-slash and mixed spellings are Windows syntax only.
        let win_only_remote = [
            "//evil/share/a",
            r"\/evil\share\a",
            r"/\evil\share\a",
            "//?/UNC/evil/share/a",
            r"\\?/UNC/evil/share/a",
            "/??/UNC/evil/share/a",
            r"\??/UNC/evil/share/a",
        ];
        for s in win_only_remote {
            let k = classify_unc(s);
            if cfg!(windows) {
                assert_eq!(k, UncKind::Remote("evil".into()), "{s}");
                assert!(network_probe_allowed(s).is_err(), "{s}");
            } else if !s.starts_with('\\') {
                assert_eq!(k, UncKind::Local, "{s}");
            }
        }
        let win_only_device = ["//./UNC/evil/share/a", "//./PhysicalDrive0"];
        for s in win_only_device {
            let k = classify_unc(s);
            if cfg!(windows) {
                assert_eq!(k, UncKind::Device, "{s}");
            } else {
                assert_eq!(k, UncKind::Local, "{s}");
            }
        }
        // The NT-namespace drive form is local, like `\\?\C:\`.
        assert_eq!(classify_unc(r"\??\C:\x"), UncKind::VerbatimLocal);
    }

    /// Rust's own parser disagrees with Win32 on two of these, which is why
    /// `classify_unc` is hand-rolled. Pinned so the next person tempted to
    /// "simplify" it onto `Path::components()` sees why not.
    #[test]
    #[cfg(windows)]
    fn rusts_path_parser_misreads_two_share_spellings() {
        use std::path::{Component, Prefix};
        let first = |s: &'static str| Path::new(s).components().next();
        // Win32 turns this into `\\?\UNC\evil\share`; Rust thinks the host is `?`.
        match first("//?/UNC/evil/share") {
            Some(Component::Prefix(p)) => {
                assert!(matches!(p.kind(), Prefix::UNC(h, _) if h == "?"))
            }
            other => panic!("{other:?}"),
        }
        // Not even absolute to Rust, yet `metadata` on it reaches the share.
        assert!(!Path::new(r"\??\UNC\evil\share").is_absolute());
        assert!(matches!(
            first(r"\??\UNC\evil\share"),
            Some(Component::RootDir)
        ));
    }

    #[test]
    #[cfg(windows)]
    fn dos_device_names_are_refused() {
        for s in [
            r"C:\x\COM1",
            r"C:\x\nul.txt",
            r"C:\x\CON",
            r"C:\x\lpt9.log",
            r"C:\x\conin$",
            r"C:\x\AUX ",
        ] {
            assert!(network_probe_allowed(s).is_err(), "{s}");
        }
        for s in [r"C:\x\COM10", r"C:\x\console.log", r"C:\x\nullable.rs"] {
            assert!(network_probe_allowed(s).is_ok(), "{s}");
        }
    }

    /// A fake filesystem for `walk_links`: `links` maps a path to its target.
    fn fake_walk(path: &str, links: &[(&str, &str)]) -> Result<PathBuf, String> {
        let map: std::collections::HashMap<PathBuf, PathBuf> = links
            .iter()
            .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
            .collect();
        let touched = std::cell::RefCell::new(Vec::<PathBuf>::new());
        let result = walk_links(
            Path::new(path),
            |p| {
                touched.borrow_mut().push(p.to_path_buf());
                Ok(map.contains_key(p))
            },
            |p| Ok(map[p].clone()),
        );
        // Whatever happened, nothing on a share was ever asked about.
        for p in touched.borrow().iter() {
            assert!(
                network_probe_allowed(&p.to_string_lossy()).is_ok(),
                "walk touched {}",
                p.display()
            );
        }
        result
    }

    #[test]
    fn a_link_to_a_share_is_refused_without_being_followed() {
        let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
        let link = format!("{root}{}docs", std::path::MAIN_SEPARATOR);
        let err = fake_walk(
            &format!("{link}{}a.md", std::path::MAIN_SEPARATOR),
            &[(&link, r"\\evil\share")],
        )
        .expect_err("link to a share");
        assert!(err.contains("network"), "{err}");
    }

    #[test]
    fn a_chain_of_local_links_ending_on_a_share_is_refused() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
        let a = format!("{root}{sep}a");
        let b = format!("{root}{sep}b");
        // a -> b (relative, local), b -> \\?\UNC\evil\share
        let err = fake_walk(
            &format!("{a}{sep}x.md"),
            &[(&a, "b"), (&b, r"\\?\UNC\evil\share")],
        )
        .expect_err("chain to a share");
        assert!(err.contains("network"), "{err}");
    }

    #[test]
    fn local_links_are_resolved_and_loops_are_bounded() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
        let link = format!("{root}{sep}link");
        let got = fake_walk(&format!("{link}{sep}f.md"), &[(&link, "real")]).unwrap();
        assert_eq!(got, Path::new(root).join("real").join("f.md"));

        let loop_ = format!("{root}{sep}loop");
        let err = fake_walk(&format!("{loop_}{sep}f"), &[(&loop_, "loop")]).unwrap_err();
        assert!(err.contains("too many"), "{err}");
    }

    /// A junction needs no privilege to create, unlike a symlink, so this
    /// pins against the real filesystem that `symlink_metadata` reports one
    /// as a link — the property `walk_links` rests on.
    #[test]
    #[cfg(windows)]
    fn a_junction_is_seen_as_a_link() {
        let dir = std::env::temp_dir().join(format!("ymux-junction-{}", std::process::id()));
        let target = dir.join("target");
        let junction = dir.join("j");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f.md"), b"x").unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");

        assert!(is_link_on_disk(&junction).unwrap());
        let resolved = resolve_local(&junction.join("f.md")).unwrap();
        assert!(!resolved.starts_with(&junction), "{}", resolved.display());
        assert!(resolved.ends_with("f.md"));
        assert!(probe_one("j/f.md", Some(&dir.to_string_lossy())).is_some());

        std::fs::remove_dir(&junction).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_relative_join_result_is_refused() {
        // A relative cwd would resolve against ymux's own working directory.
        assert!(resolve_local(Path::new("repo/src/a.rs")).is_err());
    }

    #[test]
    fn expand_joins_a_relative_path_onto_the_cwd() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let got = expand("src/main.ts", Some(&cwd), None).unwrap();
        assert_eq!(got, cwd.join("src/main.ts"));
    }

    #[test]
    fn expand_refuses_a_relative_path_with_no_cwd() {
        assert!(expand("src/main.ts", None, None).is_none());
    }

    #[test]
    fn expand_leaves_an_absolute_path_alone() {
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert_eq!(expand(abs, None, None).unwrap(), PathBuf::from(abs));
    }

    #[test]
    #[cfg(windows)]
    fn expand_treats_a_driveless_root_as_drive_relative() {
        // Windows' own rule, and `Path::join` implements it: the cwd's drive
        // survives, everything after it is replaced.
        let cwd = PathBuf::from(r"D:\Git\ymux");
        assert_eq!(
            expand("/usr/lib", Some(&cwd), None).unwrap(),
            PathBuf::from("D:/usr/lib")
        );
        assert_eq!(
            expand(r"\srv\x", Some(&cwd), None).unwrap(),
            PathBuf::from(r"D:\srv\x")
        );
    }

    #[test]
    fn expand_substitutes_the_home_directory() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        });
        assert_eq!(
            expand("~/.claude/settings.json", None, Some(&home)).unwrap(),
            home.join(".claude/settings.json")
        );
        assert_eq!(expand("~", None, Some(&home)).unwrap(), home);
        assert!(expand("~/x", None, None).is_none());
    }

    #[test]
    fn expand_does_not_treat_a_tilde_name_as_home() {
        // `~foo` is shell syntax for another user's home, which this does not
        // implement; it must stay a literal relative name.
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let home = PathBuf::from("/home/me");
        assert_eq!(
            expand("~other/x", Some(&cwd), Some(&home)).unwrap(),
            cwd.join("~other/x")
        );
    }

    #[test]
    fn strip_verbatim_rewrites_both_canonical_forms() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\nas\projects\a.txt")),
            PathBuf::from(r"\\nas\projects\a.txt")
        );
    }

    #[test]
    fn strip_verbatim_leaves_everything_else_alone() {
        assert_eq!(
            strip_verbatim(Path::new("/usr/lib/x")),
            PathBuf::from("/usr/lib/x")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        // Not the drive form, so unwrapping it would change its meaning.
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\Volume{abc}\x")),
            PathBuf::from(r"\\?\Volume{abc}\x")
        );
    }

    fn reveals(name: &str) -> bool {
        should_reveal(Path::new(name), false, false)
    }

    #[test]
    fn executables_and_scripts_are_revealed_not_run() {
        for name in [
            // What the old denylist covered.
            "evil.bat",
            "evil.BAT",
            "setup.exe",
            "a.cmd",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.lnk",
            "a.url",
            "a.reg",
            "a.app",
            "a.command",
            "a.msi",
            "a.appref-ms",
            // The review's examples, which the denylist let through.
            "a.sh",
            "a.py",
            "a.pyw",
            "a.settingcontent-ms",
            "a.appinstaller",
            "a.application",
            "a.chm",
            "a.iso",
            "a.vhd",
            "a.vhdx",
            "a.xll",
            "a.wsc",
            "a.sct",
            "a.library-ms",
            "a.search-ms",
            // Other interpreters and loaders.
            "a.rb",
            "a.pl",
            "a.php",
            "a.jar",
            "a.jsx",
            "a.sln",
            "a.csproj",
            "a.docm",
            "a.xlsm",
            "a.doc",
            "a.xml",
            "a.csv",
        ] {
            assert!(reveals(name), "{name}");
        }
    }

    /// The allowlist compares the extension exactly, so every respelling
    /// Win32 would quietly turn back into `.bat` fails closed.
    #[test]
    fn respellings_of_a_runnable_name_are_revealed() {
        for name in [
            "evil.bat.",   // Win32 strips trailing dots
            "evil.bat ",   // ...and trailing spaces
            "evil.bat. .", // ...in any mix
            "evil.Bat",
            "evil.exe:x.txt", // an ADS on an .exe, dressed as .txt
            "a.txt:evil.exe", // the review's ADS example
            "a.txt:",
            "Makefile", // extensionless: no allowlisted type to open with
            "evil",
        ] {
            assert!(reveals(name), "{name:?}");
        }
    }

    #[test]
    fn allowlisted_documents_are_opened_in_any_case() {
        for name in [
            "notes.md",
            "NOTES.MD",
            "a.rs",
            "a.ts",
            "cfg.toml",
            "photo.png",
            "Photo.JPG",
            "report.pdf",
            "sheet.xlsx",
            "index.html",
            "/srv/x/y.json",
        ] {
            assert!(!reveals(name), "{name}");
        }
        if cfg!(windows) {
            // The drive colon is not in the file name, so it is not an ADS.
            assert!(!reveals(r"C:\x\y.txt"));
        }
    }

    /// macOS `open` runs an extensionless executable in Terminal, so the
    /// execute bit wins over any extension.
    #[test]
    fn an_executable_bit_always_reveals() {
        assert!(should_reveal(Path::new("tool"), false, true));
        assert!(should_reveal(Path::new("notes.txt"), false, true));
        // A directory's search bit is not an execute bit.
        assert!(!should_reveal(Path::new("scripts"), true, false));
    }

    #[test]
    #[cfg(unix)]
    fn exec_bit_reads_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ymux-execbit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("tool");
        std::fs::write(&f, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!exec_bit(&std::fs::metadata(&f).unwrap()));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        let md = std::fs::metadata(&f).unwrap();
        assert!(exec_bit(&md));
        assert!(should_reveal(&f, false, exec_bit(&md)));
        assert!(!exec_bit(&std::fs::metadata(&dir).unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A macOS `.app` (and `.pkg`, `.workflow`, `.framework`) is a
    /// **directory**, and `open Foo.app` launches it. Any directory whose
    /// name has an extension is revealed, which also covers Windows'
    /// `folder.{CLSID}` shell junctions.
    #[test]
    fn a_bundle_is_a_directory_and_must_still_be_revealed() {
        for name in [
            "Foo.app",
            "Installer.pkg",
            "x.mpkg",
            "y.workflow",
            "z.framework",
            "q.{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
        ] {
            assert!(
                should_reveal(Path::new(name), true, false),
                "{name} is a directory that would otherwise be launched"
            );
        }
    }

    #[test]
    fn plain_directories_are_opened() {
        assert!(!should_reveal(Path::new("/srv/project"), true, false));
        assert!(!should_reveal(Path::new("scripts"), true, false));
        assert!(!should_reveal(Path::new(".git"), true, false));
        // A filesystem root has no name, and is still a folder to show.
        let root = if cfg!(windows) { r"C:\" } else { "/" };
        assert!(!should_reveal(Path::new(root), true, false));
    }

    /// The review says resolution canonicalises case, trailing dots and ADS
    /// before the reveal decision ever sees the name. Proven here on the real
    /// filesystem: what the frontend is handed back — and so what `open_path`
    /// judges — is the canonical name.
    #[test]
    #[cfg(windows)]
    fn resolution_canonicalises_respellings_before_the_decision() {
        let dir = std::env::temp_dir().join(format!("ymux-canon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("evil.bat"), b"@echo pwned").unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let cwd = dir.to_string_lossy().into_owned();

        for spelled in ["evil.bat.", "EVIL.BAT", "evil.bat. ."] {
            let got = probe_one(spelled, Some(&cwd)).expect(spelled);
            assert!(
                got.absolute.ends_with("evil.bat"),
                "{spelled} -> {}",
                got.absolute
            );
            assert!(
                should_reveal(Path::new(&got.absolute), got.is_dir, false),
                "{spelled}"
            );
        }
        // An alternate data stream that does not exist is not a link at all.
        assert!(probe_one("a.txt:evil.exe", Some(&cwd)).is_none());
        // One that does exist is NOT canonicalised away — `canonicalize`
        // keeps the `:evil.exe` (measured) — so it is the colon rule in
        // `should_reveal` that stops it, not resolution.
        std::fs::write(dir.join("a.txt:evil.exe"), b"MZ").unwrap();
        let got = probe_one("a.txt:evil.exe", Some(&cwd)).expect("existing ADS");
        assert!(got.absolute.ends_with("a.txt:evil.exe"), "{}", got.absolute);
        assert!(should_reveal(Path::new(&got.absolute), got.is_dir, false));
        // ...and the same for a stream named like a document on an `.exe`.
        std::fs::write(dir.join("evil.bat:x.txt"), b"x").unwrap();
        let got = probe_one("evil.bat:x.txt", Some(&cwd)).expect("existing ADS");
        assert!(should_reveal(Path::new(&got.absolute), got.is_dir, false));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_open_requires_an_absolute_path() {
        // Relative names beginning with `-` would become flags to macOS `open`.
        assert!(validate_open("-R").is_err());
        assert!(validate_open("src/main.ts").is_err());
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert!(validate_open(abs).is_ok());
    }

    #[test]
    fn validate_open_accepts_a_unc_share_on_any_target() {
        // An explicit click is the consent the hover probe lacks, so a share
        // the user is genuinely working on stays openable.
        assert!(validate_open(r"\\nas\projects\a.txt").is_ok());
        assert!(validate_open(r"\\?\C:\x").is_ok());
    }

    #[test]
    fn validate_open_refuses_the_device_namespace_and_junk() {
        assert!(validate_open(r"\\.\PhysicalDrive0").is_err());
        assert!(validate_open("").is_err());
        assert!(validate_open("/x/\u{7}y").is_err());
    }

    #[test]
    fn probe_one_finds_a_real_file_and_directory() {
        let dir = std::env::temp_dir().join(format!("ymux-fspath-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.txt"), b"hi").unwrap();
        let cwd = dir.to_string_lossy().into_owned();

        let file = probe_one("sub/a.txt", Some(&cwd)).expect("relative file resolves");
        assert!(!file.is_dir);
        assert!(file.absolute.ends_with("a.txt"));
        // Absolute, so it can be opened without the cwd.
        assert!(Path::new(&file.absolute).is_absolute());

        let sub = probe_one("sub", Some(&cwd)).expect("relative dir resolves");
        assert!(sub.is_dir);

        assert!(probe_one("sub/missing.txt", Some(&cwd)).is_none());
        // No cwd, so a relative candidate cannot be placed at all.
        assert!(probe_one("sub/a.txt", None).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn probe_batch_answers_every_input_in_order() {
        let got = probe_batch(
            vec!["and/or".into(), "".into(), "n/a".into()],
            Some("/definitely/not/here".into()),
        );
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|r| r.is_none()));
    }

    #[test]
    fn probe_batch_pads_past_the_cap() {
        let raws: Vec<String> = (0..MAX_BATCH + 5).map(|i| format!("d{i}/f")).collect();
        let want = raws.len();
        assert_eq!(probe_batch(raws, None).len(), want);
    }
}
