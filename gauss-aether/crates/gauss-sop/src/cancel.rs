//! [`CancelHandle`] — minimal cooperative cancellation primitive.
//!
//! Sprint 14 §1 keeps the SOP engine independent of `gaussclaw-agent`
//! (which lives in a downstream layer). The shape mirrors
//! `gaussclaw_agent::CancelHandle`: a shared atomic flag the engine
//! checks at trigger-poll boundaries. A future commit can implement
//! `From<gaussclaw_agent::CancelHandle> for CancelHandle` in the
//! agent crate to bridge them without flipping the dep direction.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A cooperative cancellation flag. Cheap to clone.
#[derive(Clone, Default, Debug)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    /// Fresh handle; `is_cancelled()` returns `false` until
    /// [`Self::request_cancel`] is called on any clone.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Flip the flag. Any future poll observing
    /// [`Self::is_cancelled`] returns `true`.
    pub fn request_cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Read the current flag.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_handle_is_not_cancelled() {
        assert!(!CancelHandle::new().is_cancelled());
    }

    #[test]
    fn request_cancel_sets_flag_across_clones() {
        let a = CancelHandle::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        assert!(!b.is_cancelled());
        b.request_cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }
}
