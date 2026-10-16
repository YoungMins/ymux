//! In-process broadcast of finished records.
//!
//! The hook point for the webhook dispatcher: the activity worker publishes
//! every `turn_end` and `permission` (attention) record here after it is
//! stored, and a subscriber turns it into a delivery. Nothing here does I/O.
//! Records are already redacted when they arrive.

use super::store::Record;
use parking_lot::Mutex;
use std::sync::Arc;

/// Called on the worker thread: must not block. A subscriber that needs to do
/// slow work queues it elsewhere.
pub type Listener = Arc<dyn Fn(&Record) + Send + Sync>;

#[derive(Default)]
pub struct ActivityHub {
    listeners: Mutex<Vec<Listener>>,
}

impl ActivityHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, l: Listener) {
        self.listeners.lock().push(l);
    }

    pub fn len(&self) -> usize {
        self.listeners.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.listeners.lock().is_empty()
    }

    /// Call every listener. The list is copied first so a listener may
    /// subscribe without deadlocking.
    pub fn publish(&self, rec: &Record) {
        let listeners: Vec<Listener> = self.listeners.lock().clone();
        for l in listeners {
            l(rec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_listener_sees_a_published_record() {
        let hub = ActivityHub::new();
        assert!(hub.is_empty());
        let n = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let n = n.clone();
            hub.subscribe(Arc::new(move |r| {
                assert_eq!(r.kind, "turn_end");
                n.fetch_add(1, Ordering::SeqCst);
            }));
        }
        hub.publish(&Record::new(1, "turn_end", serde_json::Value::Null));
        assert_eq!(n.load(Ordering::SeqCst), 2);
        assert_eq!(hub.len(), 2);
    }
}
