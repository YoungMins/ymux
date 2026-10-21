//! Where each pane's PTY output goes: one subscriber per pane, keyed by the
//! subscriber's own id so a late unsubscribe from a pane's previous life
//! cannot cut off the one that replaced it (pure; the desktop side stores a
//! Tauri `Channel` as `C`).
//!
//! Output is sent through a channel rather than an event because an event
//! payload is JSON: a `Vec<u8>` goes out as a number array (`[27,91,…]`),
//! about four times the bytes, parsed back on the other side. A channel
//! carries the raw bytes as an `ArrayBuffer`.

use std::collections::HashMap;

use uuid::Uuid;

#[derive(Debug)]
pub struct Routes<C> {
    map: HashMap<Uuid, (u32, C)>,
}

impl<C> Default for Routes<C> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<C> Routes<C> {
    /// Route `pane`'s output to `sink` (subscriber id `sub`), replacing any
    /// earlier subscriber.
    pub fn subscribe(&mut self, pane: Uuid, sub: u32, sink: C) {
        self.map.insert(pane, (sub, sink));
    }

    /// Stop routing `pane` to subscriber `sub`. A different (newer)
    /// subscriber is left alone. Returns whether anything was removed.
    pub fn unsubscribe(&mut self, pane: Uuid, sub: u32) -> bool {
        if self.map.get(&pane).is_some_and(|(s, _)| *s == sub) {
            self.map.remove(&pane);
            true
        } else {
            false
        }
    }

    /// Forget `pane` whoever subscribed (the pane was killed).
    pub fn remove(&mut self, pane: Uuid) {
        self.map.remove(&pane);
    }

    pub fn get(&self, pane: Uuid) -> Option<&C> {
        self.map.get(&pane).map(|(_, c)| c)
    }
}

/// Header naming the pane of a raw-body `write_pane` (the body is the bytes
/// themselves, so the id cannot ride in it).
pub const PANE_HEADER: &str = "x-ymux-pane";

/// The pane a raw-body write is for: the [`PANE_HEADER`] value as a UUID.
pub fn write_target(header: Option<&str>) -> Option<Uuid> {
    header.and_then(|h| Uuid::parse_str(h.trim()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_target_needs_a_well_formed_pane_id() {
        let id = Uuid::new_v4();
        assert_eq!(write_target(Some(&id.to_string())), Some(id));
        assert_eq!(write_target(Some(&format!(" {id} "))), Some(id));
        assert_eq!(write_target(Some("not-a-uuid")), None);
        assert_eq!(write_target(None), None);
    }

    #[test]
    fn a_newer_subscriber_replaces_and_survives_a_stale_unsubscribe() {
        let pane = Uuid::new_v4();
        let mut r = Routes::default();
        r.subscribe(pane, 1, "old");
        r.subscribe(pane, 2, "new");
        assert_eq!(r.get(pane), Some(&"new"));
        // The first life's teardown arrives late.
        assert!(!r.unsubscribe(pane, 1));
        assert_eq!(r.get(pane), Some(&"new"));
        assert!(r.unsubscribe(pane, 2));
        assert_eq!(r.get(pane), None);
    }

    #[test]
    fn panes_are_independent_and_remove_ignores_the_subscriber() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut r = Routes::default();
        r.subscribe(a, 7, 'a');
        r.subscribe(b, 7, 'b');
        r.remove(a);
        assert_eq!(r.get(a), None);
        assert_eq!(r.get(b), Some(&'b'));
        assert!(!r.unsubscribe(a, 7));
    }
}
