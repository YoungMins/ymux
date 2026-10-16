//! Activity log: zero-token capture of what agents did, per turn.
//!
//! Pure (no Tauri, no desktop gate) so it is tested on Linux CI. The wiring
//! into the hook receiver and a worker thread lives elsewhere; this module
//! only turns events into clipped, redactable facts and stores them.
//!
//! * [`redact`]: secret patterns, sensitive paths, char-safe clipping.
//! * [`extract`]: Claude Code hook JSON -> [`extract::ActivityRaw`].
//! * [`codex`]: Codex rollout JSONL lines -> [`codex::CodexItem`].
//! * [`turn`]: per-(pane, session) turn state machine -> [`turn::TurnFacts`].
//! * [`store`]: daily JSONL log with caps and retention.
//! * [`worker`]: the off-hot-path consumer (redact, context, git stats,
//!   turn tracking, store, hub).
//! * [`tail`]: incremental reader of a Codex rollout file.
//! * [`hub`]: in-process broadcast of finished records (webhook hook point).
//! * `runtime` (desktop): thread and Tauri glue.

pub mod codex;
pub mod extract;
pub mod gitstat;
pub mod hub;
pub mod redact;
pub mod store;
pub mod tail;
pub mod turn;
pub mod worker;

#[cfg(feature = "desktop")]
pub mod runtime;

/// What crosses the bounded channel from a producer to the worker. Small and
/// already clipped: no `serde_json::Value` is ever queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityMsg {
    /// A Claude Code hook event (accepted path only).
    Hook(extract::ActivityRaw),
    /// A line of a Codex rollout, with the session the tracker bound the
    /// pane to and how that binding was made.
    Codex {
        session: String,
        source: crate::agent_sessions::IdSource,
        item: codex::CodexItem,
    },
    /// The pane was killed: drop whatever it had open.
    Forget,
}
