//! Pane list bookkeeping: which status transitions raise a notification, and
//! the one-frame-a-second throttle on the `panes` push. Pure.

use std::collections::HashMap;

use super::proto::Pane;

/// At most one notification per pane in this window.
pub const NOTICE_GAP_MS: u64 = 30_000;
/// At most one `panes` frame per second.
pub const FRAME_GAP_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// `waiting` or `done`.
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    pub pane: String,
}

#[derive(Debug, Default)]
pub struct Tracker {
    status: HashMap<String, String>,
    last_notice: HashMap<String, u64>,
    seeded: bool,
}

impl Tracker {
    /// Compare `panes` with the previous list and return the notifications
    /// that are due. The first list only seeds the state (panes that were
    /// already waiting when the link came up are not news). A pane entering
    /// `waiting` notifies as `waiting`; one going `working` -> `done` as
    /// `done`. `can_send` false (no server connection) returns nothing and
    /// does not start the per-pane gap, so the transition is simply missed
    /// rather than reported late.
    pub fn observe(
        &mut self,
        panes: &[Pane],
        now_ms: u64,
        waiting: bool,
        done: bool,
        can_send: bool,
    ) -> Vec<Notice> {
        let mut out = Vec::new();
        for p in panes {
            let prev = self.status.get(&p.pane).map(String::as_str);
            let kind = match (self.seeded, prev, p.status.as_str()) {
                (true, prev, "waiting") if prev != Some("waiting") && waiting => Some("waiting"),
                (true, Some("working"), "done") if done => Some("done"),
                _ => None,
            };
            if let (Some(kind), true) = (kind, can_send) {
                let recent = self
                    .last_notice
                    .get(&p.pane)
                    .is_some_and(|t| now_ms < t.saturating_add(NOTICE_GAP_MS));
                if !recent {
                    self.last_notice.insert(p.pane.clone(), now_ms);
                    out.push(Notice {
                        kind,
                        title: if p.title.is_empty() {
                            "Agent".to_string()
                        } else {
                            p.title.clone()
                        },
                        body: p.ws.clone(),
                        pane: p.pane.clone(),
                    });
                }
            }
        }
        self.status = panes
            .iter()
            .map(|p| (p.pane.clone(), p.status.clone()))
            .collect();
        self.last_notice.retain(|id, t| {
            self.status.contains_key(id) || now_ms < t.saturating_add(NOTICE_GAP_MS)
        });
        self.seeded = true;
        out
    }
}

/// Holds the newest serialized pane list and releases it at most once per
/// [`FRAME_GAP_MS`], and only when it differs from what was last released.
#[derive(Debug, Default)]
pub struct Throttle {
    latest: String,
    sent: String,
    sent_ms: u64,
}

impl Throttle {
    pub fn set(&mut self, json: String) {
        self.latest = json;
    }

    /// The frame to send now, if one is due.
    pub fn take(&mut self, now_ms: u64) -> Option<String> {
        if self.latest.is_empty()
            || self.latest == self.sent
            || now_ms < self.sent_ms.saturating_add(FRAME_GAP_MS)
        {
            return None;
        }
        self.sent = self.latest.clone();
        self.sent_ms = now_ms;
        Some(self.sent.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, status: &str) -> Pane {
        Pane {
            pane: id.into(),
            ws: "work".into(),
            title: format!("title {id}"),
            kind: "claude".into(),
            status: status.into(),
            since_ms: 0,
            queued: 0,
        }
    }

    #[test]
    fn first_list_only_seeds() {
        let mut t = Tracker::default();
        assert!(t
            .observe(&[pane("a", "waiting")], 0, true, true, true)
            .is_empty());
        // Still waiting: not a transition.
        assert!(t
            .observe(&[pane("a", "waiting")], 5_000, true, true, true)
            .is_empty());
    }

    #[test]
    fn entering_waiting_notifies_once() {
        let mut t = Tracker::default();
        t.observe(&[pane("a", "working")], 0, true, true, true);
        let n = t.observe(&[pane("a", "waiting")], 1_000, true, true, true);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "waiting");
        assert_eq!(n[0].pane, "a");
        assert_eq!(n[0].title, "title a");
        assert_eq!(n[0].body, "work");
        assert!(t
            .observe(&[pane("a", "waiting")], 2_000, true, true, true)
            .is_empty());
    }

    #[test]
    fn a_new_pane_that_is_already_waiting_notifies() {
        let mut t = Tracker::default();
        t.observe(&[pane("a", "idle")], 0, true, true, true);
        let n = t.observe(
            &[pane("a", "idle"), pane("b", "waiting")],
            1_000,
            true,
            true,
            true,
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].pane, "b");
    }

    #[test]
    fn done_notifies_only_from_working() {
        let mut t = Tracker::default();
        t.observe(
            &[pane("a", "working"), pane("b", "idle")],
            0,
            true,
            true,
            true,
        );
        let n = t.observe(
            &[pane("a", "done"), pane("b", "done")],
            1_000,
            true,
            true,
            true,
        );
        assert_eq!(n.len(), 1);
        assert_eq!((n[0].kind, n[0].pane.as_str()), ("done", "a"));
    }

    #[test]
    fn switches_turn_kinds_off() {
        let mut t = Tracker::default();
        t.observe(
            &[pane("a", "working"), pane("b", "working")],
            0,
            false,
            false,
            true,
        );
        let n = t.observe(
            &[pane("a", "waiting"), pane("b", "done")],
            1_000,
            false,
            false,
            true,
        );
        assert!(n.is_empty());
        let mut t = Tracker::default();
        t.observe(
            &[pane("a", "working"), pane("b", "working")],
            0,
            true,
            false,
            true,
        );
        let n = t.observe(
            &[pane("a", "waiting"), pane("b", "done")],
            1_000,
            true,
            false,
            true,
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "waiting");
    }

    #[test]
    fn at_most_one_per_pane_per_30_seconds() {
        let mut t = Tracker::default();
        t.observe(&[pane("a", "working")], 0, true, true, true);
        assert_eq!(
            t.observe(&[pane("a", "waiting")], 1_000, true, true, true)
                .len(),
            1
        );
        t.observe(&[pane("a", "working")], 2_000, true, true, true);
        // Back to waiting 10 s after the first: suppressed.
        assert!(t
            .observe(&[pane("a", "waiting")], 11_000, true, true, true)
            .is_empty());
        t.observe(&[pane("a", "working")], 12_000, true, true, true);
        // 30 s after the first: allowed again.
        assert_eq!(
            t.observe(&[pane("a", "waiting")], 31_000, true, true, true)
                .len(),
            1
        );
        // A different pane has its own gap.
        let n = t.observe(
            &[pane("a", "waiting"), pane("b", "waiting")],
            32_000,
            true,
            true,
            true,
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].pane, "b");
    }

    #[test]
    fn offline_misses_the_transition_without_starting_the_gap() {
        let mut t = Tracker::default();
        t.observe(&[pane("a", "working")], 0, true, true, true);
        assert!(t
            .observe(&[pane("a", "waiting")], 1_000, true, true, false)
            .is_empty());
        t.observe(&[pane("a", "working")], 2_000, true, true, true);
        assert_eq!(
            t.observe(&[pane("a", "waiting")], 3_000, true, true, true)
                .len(),
            1
        );
    }

    #[test]
    fn empty_title_gets_a_neutral_one() {
        let mut t = Tracker::default();
        t.observe(&[pane("a", "working")], 0, true, true, true);
        let mut p = pane("a", "waiting");
        p.title.clear();
        let n = t.observe(&[p], 1_000, true, true, true);
        assert_eq!(n[0].title, "Agent");
    }

    #[test]
    fn throttle_releases_changes_at_most_once_a_second() {
        let mut th = Throttle::default();
        assert_eq!(th.take(0), None);
        th.set("one".into());
        assert_eq!(th.take(5_000).as_deref(), Some("one"));
        th.set("two".into());
        assert_eq!(th.take(5_500), None);
        assert_eq!(th.take(5_999), None);
        assert_eq!(th.take(6_000).as_deref(), Some("two"));
        // Unchanged content is not resent.
        assert_eq!(th.take(9_000), None);
        th.set("two".into());
        assert_eq!(th.take(9_000), None);
        // Only the newest of several changes goes out.
        th.set("three".into());
        th.set("four".into());
        assert_eq!(th.take(10_000).as_deref(), Some("four"));
    }
}
