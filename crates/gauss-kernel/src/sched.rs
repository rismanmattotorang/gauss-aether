//! Three-plane scheduler — Axiom 4, Theorem T4.
//!
//! Phase 0 ships a minimal but real token-bucket per plane:
//!
//! * Per-plane state lives in its own atomic / lock — no cross-plane shared
//!   counter, so starvation freedom holds by construction.
//! * `try_acquire` is non-blocking and returns whether a token was granted.
//! * Refill is amortised on each `try_acquire`, computed from a monotonic
//!   clock — no background thread, no time skew.
//!
//! Phase 1 ports this to a lock-free `AtomicU64` cell encoding both tokens
//! and last-refill timestamp; the current `parking_lot::Mutex` implementation
//! is simpler and easier to property-test for Phase 0.

use core::time::Duration;
use std::time::Instant;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The three scheduler planes. Per `SPECS.md` §4.3.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    /// Synchronous user-driven turns.
    Conversation,
    /// Scheduled / autonomous turns.
    Daemon,
    /// Human-in-the-loop approval round-trips.
    Approval,
}

/// A single token-bucket plane pool.
///
/// Worst-case wait time for `try_acquire` to succeed under sustained pressure
/// is `capacity / refill_rate` — the bound that drives Theorem T4 starvation
/// freedom.
#[derive(Debug)]
pub struct PlanePool {
    state: Mutex<BucketState>,
    /// Tokens per second added back to the bucket.
    refill_per_sec: f64,
    /// Bucket capacity (maximum tokens held at any instant).
    capacity: f64,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl PlanePool {
    /// Construct a new plane pool.
    ///
    /// # Panics
    /// Panics if `capacity` or `refill_per_sec` is non-finite or negative.
    #[must_use]
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        assert!(
            capacity.is_finite() && capacity > 0.0,
            "capacity must be positive and finite",
        );
        assert!(
            refill_per_sec.is_finite() && refill_per_sec > 0.0,
            "refill_per_sec must be positive and finite",
        );
        Self {
            state: Mutex::new(BucketState {
                tokens: capacity,
                last_refill: Instant::now(),
            }),
            refill_per_sec,
            capacity,
        }
    }

    /// Refill amortised against elapsed wallclock and try to take one token.
    /// Returns `true` iff a token was granted.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    /// Same as [`Self::try_acquire`] but with an explicit `now` for tests.
    pub fn try_acquire_at(&self, now: Instant) -> bool {
        let mut state = self.state.lock();
        let elapsed = now
            .saturating_duration_since(state.last_refill)
            .as_secs_f64();
        state.tokens = elapsed
            .mul_add(self.refill_per_sec, state.tokens)
            .min(self.capacity);
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Current token count (for diagnostics / metrics only).
    pub fn tokens(&self) -> f64 {
        self.state.lock().tokens
    }

    /// Upper bound on wait time for one token under sustained demand.
    /// This is the `B / ρ` term from Theorem T4.
    #[must_use]
    pub fn worst_case_wait(&self) -> Duration {
        Duration::from_secs_f64(self.capacity / self.refill_per_sec)
    }
}

/// The three planes wired together. Each plane has an independent bucket; no
/// cross-plane shared state.
#[derive(Debug)]
pub struct Planes {
    /// Synchronous user turn plane.
    pub conversation: PlanePool,
    /// Scheduled / autonomous daemon plane.
    pub daemon: PlanePool,
    /// HITL approval plane.
    pub approval: PlanePool,
}

impl Planes {
    /// Construct the three planes with the default `SPECS.md` §4.3 capacities.
    ///
    /// Refill rates are configurable per tenant in Phase 1; the defaults here
    /// are intentionally conservative so tests run deterministically.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self {
            conversation: PlanePool::new(32.0, 8.0),
            daemon: PlanePool::new(16.0, 2.0),
            approval: PlanePool::new(64.0, 4.0),
        }
    }

    /// Look up the pool for a plane.
    #[must_use]
    pub const fn pool(&self, plane: Plane) -> &PlanePool {
        match plane {
            Plane::Conversation => &self.conversation,
            Plane::Daemon => &self.daemon,
            Plane::Approval => &self.approval,
        }
    }
}

/// Re-export for documentation continuity with the SPECS naming.
pub type TokenBucket = PlanePool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bucket_refuses_until_refilled() {
        let pool = PlanePool::new(2.0, 1.0);
        let t0 = Instant::now();
        assert!(pool.try_acquire_at(t0));
        assert!(pool.try_acquire_at(t0));
        assert!(
            !pool.try_acquire_at(t0),
            "third acquire at t0 must fail (bucket drained)"
        );
        let t1 = t0 + Duration::from_secs(1);
        assert!(
            pool.try_acquire_at(t1),
            "after 1s with 1 token/s, must grant"
        );
    }

    #[test]
    fn cross_plane_starvation_does_not_propagate() {
        // Drain the daemon plane completely and verify the conversation
        // plane keeps serving.
        let planes = Planes::with_defaults();
        let t0 = Instant::now();
        // Drain daemon to zero.
        while planes.daemon.try_acquire_at(t0) {}
        assert!(!planes.daemon.try_acquire_at(t0));
        // Conversation still serves.
        assert!(planes.conversation.try_acquire_at(t0));
    }

    #[test]
    fn worst_case_wait_matches_b_over_rho() {
        let pool = PlanePool::new(10.0, 2.0); // B = 10, rho = 2 => 5s
        assert_eq!(pool.worst_case_wait(), Duration::from_secs(5));
    }

    #[test]
    #[should_panic(expected = "capacity must be positive")]
    fn rejects_zero_capacity() {
        let _ = PlanePool::new(0.0, 1.0);
    }
}
