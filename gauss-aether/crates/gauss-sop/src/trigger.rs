//! [`Trigger`] — the engine's event-source trait.

use async_trait::async_trait;
use std::sync::Mutex;

use crate::cancel::CancelHandle;
use crate::event::TriggerEvent;

/// Source of [`TriggerEvent`]s. Production triggers live in adjacent
/// crates (webhook = Sprint 14 §3; MQTT / peripherals = follow-on).
#[async_trait]
pub trait Trigger: Send + Sync {
    /// Stable identifier the engine carries on every receipt.
    fn name(&self) -> &str;

    /// Block until the next event, the cancellation flag flips, or
    /// the source is permanently drained. Returning `None` signals
    /// "done" to the engine — the engine will not poll this trigger
    /// again. Trigger errors that the engine should *not* shut down
    /// over should be surfaced as [`TriggerEvent`]s with an error
    /// payload, never as `None`.
    async fn next(&mut self, cancel: CancelHandle) -> Option<TriggerEvent>;
}

/// Reference trigger backed by an in-memory queue. Tests pre-seed
/// events; the engine drains the queue then sees `None`.
///
/// Internally uses `Mutex` rather than `RefCell` so the type stays
/// `Send + Sync` (the [`Trigger`] trait requires both).
pub struct MemoryTrigger {
    name: String,
    events: Mutex<std::collections::VecDeque<TriggerEvent>>,
}

impl MemoryTrigger {
    /// Build a trigger that emits the supplied events in order.
    pub fn new(name: impl Into<String>, events: impl IntoIterator<Item = TriggerEvent>) -> Self {
        Self {
            name: name.into(),
            events: Mutex::new(events.into_iter().collect()),
        }
    }

    /// Push another event onto the back of the queue. The engine will
    /// see it after the events already queued.
    pub fn push(&self, event: TriggerEvent) {
        self.events
            .lock()
            .expect("MemoryTrigger mutex poisoned")
            .push_back(event);
    }
}

#[async_trait]
impl Trigger for MemoryTrigger {
    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self, cancel: CancelHandle) -> Option<TriggerEvent> {
        if cancel.is_cancelled() {
            return None;
        }
        self.events
            .lock()
            .expect("MemoryTrigger mutex poisoned")
            .pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_trigger_drains_queue_in_order() {
        let mut t = MemoryTrigger::new(
            "mem",
            [
                TriggerEvent::new("mem", serde_json::json!({ "n": 1 })),
                TriggerEvent::new("mem", serde_json::json!({ "n": 2 })),
            ],
        );
        let cancel = CancelHandle::new();
        let first = t.next(cancel.clone()).await.unwrap();
        let second = t.next(cancel.clone()).await.unwrap();
        let done = t.next(cancel).await;
        assert_eq!(first.payload, serde_json::json!({ "n": 1 }));
        assert_eq!(second.payload, serde_json::json!({ "n": 2 }));
        assert!(done.is_none());
    }

    #[tokio::test]
    async fn memory_trigger_returns_none_when_cancelled() {
        let mut t = MemoryTrigger::new("mem", [TriggerEvent::new("mem", serde_json::Value::Null)]);
        let cancel = CancelHandle::new();
        cancel.request_cancel();
        assert!(t.next(cancel).await.is_none());
    }
}
