//! Wall-clock abstraction. Production code uses [`SystemClock`];
//! tests use [`FixedClock`] to drive the scheduler deterministically.

use std::sync::atomic::{AtomicI64, Ordering};
use time::OffsetDateTime;

/// Anything that can answer "what UTC time is it?".
pub trait Clock: Send + Sync {
    /// Current UTC time.
    fn now(&self) -> OffsetDateTime;
}

/// Real wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Deterministic clock that exposes `set` and `advance` for tests.
/// Internally a `unix_timestamp_nanos` atomic so a fixture can mutate
/// it from one thread while the scheduler reads from another.
#[derive(Debug)]
pub struct FixedClock {
    ns: AtomicI64,
}

impl FixedClock {
    /// Build a clock pinned at `t`.
    #[must_use]
    pub fn new(t: OffsetDateTime) -> Self {
        Self {
            ns: AtomicI64::new(i64_nanos(t)),
        }
    }

    /// Build a clock pinned at the UNIX epoch.
    #[must_use]
    pub fn epoch() -> Self {
        Self::new(OffsetDateTime::UNIX_EPOCH)
    }

    /// Replace the clock's value.
    pub fn set(&self, t: OffsetDateTime) {
        self.ns.store(i64_nanos(t), Ordering::SeqCst);
    }

    /// Advance the clock by `seconds`.
    pub fn advance(&self, seconds: i64) {
        let cur = self.ns.load(Ordering::SeqCst);
        // 1e9 ns per second. Saturating in case a test asks for absurd
        // advances; the scheduler tolerates clock jumps.
        let delta = seconds.saturating_mul(1_000_000_000);
        self.ns.store(cur.saturating_add(delta), Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        // Reconstruct the wall-clock from the stored nanos. Cannot
        // overflow under any realistic test budget.
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.ns.load(Ordering::SeqCst)))
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }
}

fn i64_nanos(t: OffsetDateTime) -> i64 {
    // OffsetDateTime::unix_timestamp_nanos returns i128 to cover the
    // full year range; for our scheduler the i64 nanosecond window
    // (year 1677 → 2262) is more than enough.
    i64::try_from(t.unix_timestamp_nanos()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn system_clock_returns_a_recent_time() {
        let c = SystemClock;
        let t = c.now();
        // Should be after 2026-01-01 — sanity check.
        assert!(t.year() >= 2026, "system clock looks broken: {t:?}");
    }

    #[test]
    fn fixed_clock_round_trips() {
        let pinned = OffsetDateTime::UNIX_EPOCH + Duration::seconds(1_700_000_000);
        let c = FixedClock::new(pinned);
        assert_eq!(c.now(), pinned);
    }

    #[test]
    fn fixed_clock_advance_is_additive() {
        let c = FixedClock::epoch();
        c.advance(60);
        c.advance(30);
        assert_eq!(c.now().unix_timestamp(), 90);
    }

    #[test]
    fn fixed_clock_set_replaces() {
        let c = FixedClock::epoch();
        c.advance(1000);
        c.set(OffsetDateTime::UNIX_EPOCH + Duration::seconds(50));
        assert_eq!(c.now().unix_timestamp(), 50);
    }
}
