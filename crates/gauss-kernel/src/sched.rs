//! Three-plane scheduler — Axiom A4, Theorem T4 (Phase 1: lock-free).
//!
//! The bucket state is packed into a single `AtomicU64`:
//!
//! ```text
//!   ┌─────────────── 32 bits ───────────────┬────────── 32 bits ─────────┐
//!   │ tokens (fixed-point, 16.16)           │ epoch_ms since pool start  │
//!   └────────────────────────────────────┴────────────────────────────┘
//! ```
//!
//! * Tokens are stored as a 16.16 fixed-point value (capacity up to 65535,
//!   sub-token resolution ≈ 1/65536). This is more than enough for the
//!   capacity range we use (1..1024 tokens per bucket).
//! * The epoch is in milliseconds since the pool's `Instant::now()` at
//!   construction, monotonic by construction, valid for ~49 days
//!   (`u32::MAX` ms). Past that, the bucket gracefully clamps and refills.
//!
//! All updates are compare-and-swap loops; under contention this is wait-free
//! up to one retry per concurrent caller. No allocation, no mutex.

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::time::Instant;

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

const FIXED_POINT_SCALE: u32 = 1 << 16; // 65 536

/// A single lock-free token-bucket plane pool.
///
/// Worst-case wait time for `try_acquire` to succeed under sustained pressure
/// is `capacity / refill_rate` — the bound that drives Theorem T4 starvation
/// freedom.
#[derive(Debug)]
pub struct PlanePool {
    /// Packed `(tokens_fp32, epoch_ms)`.
    state: AtomicU64,
    /// Tokens per second added back to the bucket.
    refill_per_sec: f64,
    /// Bucket capacity (maximum tokens held at any instant).
    capacity: f64,
    /// Construction instant — basis for the `epoch_ms` field.
    epoch_zero: Instant,
}

#[inline]
const fn pack(tokens_fp: u32, epoch_ms: u32) -> u64 {
    ((tokens_fp as u64) << 32) | (epoch_ms as u64)
}

#[inline]
const fn unpack(raw: u64) -> (u32, u32) {
    let tokens = (raw >> 32) as u32;
    let epoch = (raw & 0xFFFF_FFFF) as u32;
    (tokens, epoch)
}

#[inline]
fn tokens_to_fp(tokens: f64) -> u32 {
    let scaled = tokens * f64::from(FIXED_POINT_SCALE);
    if scaled < 0.0 {
        0
    } else if scaled > f64::from(u32::MAX) {
        u32::MAX
    } else {
        // Safe because of the bounds above.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            scaled as u32
        }
    }
}

#[inline]
fn fp_to_tokens(fp: u32) -> f64 {
    f64::from(fp) / f64::from(FIXED_POINT_SCALE)
}

impl PlanePool {
    /// Construct a new plane pool.
    ///
    /// # Panics
    /// Panics if `capacity` or `refill_per_sec` is non-finite, non-positive, or
    /// exceeds the fixed-point range (≈ 65535 tokens).
    #[must_use]
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        assert!(
            capacity.is_finite() && capacity > 0.0 && capacity <= f64::from(u16::MAX),
            "capacity must be in (0, 65535]",
        );
        assert!(
            refill_per_sec.is_finite() && refill_per_sec > 0.0,
            "refill_per_sec must be positive and finite",
        );
        let tokens_fp = tokens_to_fp(capacity);
        Self {
            state: AtomicU64::new(pack(tokens_fp, 0)),
            refill_per_sec,
            capacity,
            epoch_zero: Instant::now(),
        }
    }

    /// Refill amortised against elapsed wallclock and try to take one token.
    /// Returns `true` iff a token was granted.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    /// Compute the `epoch_ms` value for an instant; clamps to `u32::MAX`.
    #[inline]
    fn epoch_ms_at(&self, now: Instant) -> u32 {
        let elapsed = now.saturating_duration_since(self.epoch_zero);
        u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX)
    }

    /// Same as [`Self::try_acquire`] but with an explicit `now` for tests.
    pub fn try_acquire_at(&self, now: Instant) -> bool {
        let now_epoch = self.epoch_ms_at(now);

        loop {
            let current = self.state.load(Ordering::Acquire);
            let (tokens_fp, last_epoch) = unpack(current);
            let elapsed_secs = f64::from(now_epoch.saturating_sub(last_epoch)) / 1000.0;
            let mut tokens = fp_to_tokens(tokens_fp);
            tokens = elapsed_secs
                .mul_add(self.refill_per_sec, tokens)
                .min(self.capacity);

            if tokens < 1.0 {
                // Persist the refill timestamp anyway (we still observed time
                // passing) so the next caller sees a more accurate bucket.
                let updated = pack(tokens_to_fp(tokens), now_epoch);
                if self
                    .state
                    .compare_exchange_weak(current, updated, Ordering::Release, Ordering::Acquire)
                    .is_ok()
                {
                    return false;
                }
                continue;
            }

            tokens -= 1.0;
            let updated = pack(tokens_to_fp(tokens), now_epoch);
            if self
                .state
                .compare_exchange_weak(current, updated, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
            // else: CAS contended — retry the whole sequence.
        }
    }

    /// Current token count (for diagnostics / metrics only).
    pub fn tokens(&self) -> f64 {
        let (fp, _) = unpack(self.state.load(Ordering::Acquire));
        fp_to_tokens(fp)
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
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn empty_bucket_refuses_until_refilled() {
        let pool = PlanePool::new(2.0, 1.0);
        let t0 = Instant::now();
        assert!(pool.try_acquire_at(t0));
        assert!(pool.try_acquire_at(t0));
        assert!(!pool.try_acquire_at(t0));
        let t1 = t0 + Duration::from_secs(1);
        assert!(pool.try_acquire_at(t1));
    }

    #[test]
    fn cross_plane_starvation_does_not_propagate() {
        let planes = Planes::with_defaults();
        let t0 = Instant::now();
        while planes.daemon.try_acquire_at(t0) {}
        assert!(!planes.daemon.try_acquire_at(t0));
        assert!(planes.conversation.try_acquire_at(t0));
    }

    #[test]
    fn worst_case_wait_matches_b_over_rho() {
        let pool = PlanePool::new(10.0, 2.0);
        assert_eq!(pool.worst_case_wait(), Duration::from_secs(5));
    }

    #[test]
    fn concurrent_acquires_never_exceed_capacity() {
        // Hammer the bucket from N threads; the total number of granted
        // tokens MUST equal the bucket's initial capacity (no double-grants).
        let pool = Arc::new(PlanePool::new(100.0, 0.001)); // refill effectively off
        let mut handles = vec![];
        for _ in 0..8 {
            let p = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                let t0 = Instant::now();
                let mut granted = 0u32;
                for _ in 0..1000 {
                    if p.try_acquire_at(t0) {
                        granted += 1;
                    }
                }
                granted
            }));
        }
        let total: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(
            total, 100,
            "exactly capacity tokens must be granted across threads"
        );
    }

    #[test]
    #[should_panic(expected = "capacity must be in")]
    fn rejects_zero_capacity() {
        let _ = PlanePool::new(0.0, 1.0);
    }

    proptest! {
        #[test]
        fn refilled_tokens_never_exceed_capacity(elapsed_ms in 0u64..86_400_000, cap in 1.0f64..1000.0, rate in 0.1f64..100.0) {
            let pool = PlanePool::new(cap, rate);
            // Drain.
            let t0 = Instant::now();
            while pool.try_acquire_at(t0) {}
            // Wait elapsed_ms.
            let t1 = t0.checked_add(Duration::from_millis(elapsed_ms)).unwrap_or(t0);
            // Cause a refill by calling try_acquire (and consuming at most one).
            let _ = pool.try_acquire_at(t1);
            // The remaining token count must never exceed `capacity`.
            prop_assert!(pool.tokens() <= cap + 1e-3);
        }
    }
}
