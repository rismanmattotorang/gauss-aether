//! `gauss-chaos` — deterministic chaos injectors (paper §XIV.A).
//!
//! Phase 10 ships a tiny chaos-engineering harness so the conformance
//! suite can validate Theorem-T1 crash semantics, Theorem-T6 stateless
//! turn scaling, and the receipt chain's tamper-evidence under
//! intermittent failures *without* killing the test process or
//! manipulating real network sockets.
//!
//! The three injectors are:
//!
//! * [`KillSwitch`] — atomically arms a flag; observable subjects poll
//!   it before every "side-effect" boundary and short-circuit when the
//!   switch is armed. Used to simulate `kill -9` at deterministic
//!   points in a turn.
//! * [`Partition`] — wraps a `tokio::sync::mpsc` pair; when partitioned,
//!   the receiver returns `None` immediately. Used to simulate network
//!   partitions in cluster mode.
//! * [`ClockSkew`] — owns a monotonic offset added to every call to
//!   `now_ms()`. Used to simulate clock skew across cluster nodes.
//!
//! The injectors compose: tests can build a [`ChaosBudget`] that holds
//! all three plus a per-injector trigger schedule, and the conformance
//! harness drives the budget through a `TurnEngine`.

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Deterministic kill switch — arm it once and every subsequent
/// [`Self::armed`] returns `true`.
#[derive(Debug, Default)]
pub struct KillSwitch {
    armed: AtomicBool,
    /// Number of times the switch was polled, for diagnostics.
    polls: AtomicU64,
}

impl KillSwitch {
    /// Build an un-armed switch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            polls: AtomicU64::new(0),
        }
    }

    /// Atomically arm the switch.
    pub fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Atomically disarm the switch.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    /// Poll the switch. Returns `true` iff the switch has been armed
    /// before this call.
    pub fn armed(&self) -> bool {
        self.polls.fetch_add(1, Ordering::AcqRel);
        self.armed.load(Ordering::Acquire)
    }

    /// Snapshot the number of polls so far.
    pub fn poll_count(&self) -> u64 {
        self.polls.load(Ordering::Acquire)
    }
}

/// Network-partition injector.
///
/// Logically wraps a (sender, receiver) pair: callers send through
/// [`Self::send_or_drop`] which drops the message when partitioned. The
/// receiver side polls [`Self::is_partitioned`].
#[derive(Debug)]
pub struct Partition<T> {
    partitioned: AtomicBool,
    queue: Mutex<Vec<T>>,
    sent: AtomicU64,
    dropped: AtomicU64,
}

impl<T> Default for Partition<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Partition<T> {
    /// Build an un-partitioned channel.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            partitioned: AtomicBool::new(false),
            queue: Mutex::new(Vec::new()),
            sent: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Trigger a partition.
    pub fn partition(&self) {
        self.partitioned.store(true, Ordering::Release);
    }

    /// Heal the partition.
    pub fn heal(&self) {
        self.partitioned.store(false, Ordering::Release);
    }

    /// True iff the channel is currently partitioned.
    pub fn is_partitioned(&self) -> bool {
        self.partitioned.load(Ordering::Acquire)
    }

    /// Send `msg`; drops it when partitioned.
    pub fn send_or_drop(&self, msg: T) {
        if self.is_partitioned() {
            self.dropped.fetch_add(1, Ordering::AcqRel);
            return;
        }
        self.queue.lock().push(msg);
        self.sent.fetch_add(1, Ordering::AcqRel);
    }

    /// Pop one message in FIFO order, or `None` if the queue is empty.
    pub fn recv(&self) -> Option<T> {
        let mut q = self.queue.lock();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }

    /// Number of messages successfully sent.
    pub fn sent_count(&self) -> u64 {
        self.sent.load(Ordering::Acquire)
    }

    /// Number of messages dropped by an active partition.
    pub fn drop_count(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }
}

/// Clock-skew injector. Reports `wall_ms() + offset_ms`.
#[derive(Debug, Default)]
pub struct ClockSkew {
    offset_ms: AtomicI64,
}

impl ClockSkew {
    /// Build with zero offset.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            offset_ms: AtomicI64::new(0),
        }
    }

    /// Add `delta_ms` to the current offset.
    pub fn add(&self, delta_ms: i64) {
        self.offset_ms.fetch_add(delta_ms, Ordering::AcqRel);
    }

    /// Replace the offset.
    pub fn set(&self, offset_ms: i64) {
        self.offset_ms.store(offset_ms, Ordering::Release);
    }

    /// Read the current offset.
    pub fn offset(&self) -> i64 {
        self.offset_ms.load(Ordering::Acquire)
    }

    /// Apply the offset to a baseline wall-clock `ms`.
    #[allow(clippy::arithmetic_side_effects)] // `i64::MIN.wrapping_neg()` is safe under `wrapping_*` semantics.
    pub fn apply(&self, baseline_ms: u64) -> u64 {
        let off = self.offset();
        if off >= 0 {
            #[allow(clippy::cast_sign_loss)]
            baseline_ms.saturating_add(off as u64)
        } else {
            #[allow(clippy::cast_sign_loss)]
            baseline_ms.saturating_sub(off.wrapping_neg() as u64)
        }
    }
}

/// Bundle of injectors a chaos test wires into a `TurnEngine`.
#[derive(Debug, Default)]
pub struct ChaosBudget {
    /// Kill switch.
    pub kill: KillSwitch,
    /// Network partition.
    pub partition: Partition<Vec<u8>>,
    /// Clock skew.
    pub clock: ClockSkew,
}

impl ChaosBudget {
    /// Build a fresh, calm budget.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            kill: KillSwitch::new(),
            partition: Partition::new(),
            clock: ClockSkew::new(),
        }
    }
}

/// Operator-readable chaos report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChaosReport {
    /// True iff the kill switch fired at any point.
    pub kill_fired: bool,
    /// Number of partition events.
    pub partition_drops: u64,
    /// Net clock-skew offset (ms).
    pub clock_offset_ms: i64,
    /// True iff the system invariants survived the chaos run.
    pub survived: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_switch_is_armed_after_arm() {
        let k = KillSwitch::new();
        assert!(!k.armed());
        k.arm();
        assert!(k.armed());
        k.disarm();
        assert!(!k.armed());
    }

    #[test]
    fn partition_drops_messages_when_partitioned() {
        let p: Partition<&'static str> = Partition::new();
        p.send_or_drop("a");
        p.partition();
        p.send_or_drop("b");
        p.heal();
        p.send_or_drop("c");
        assert_eq!(p.sent_count(), 2);
        assert_eq!(p.drop_count(), 1);
        assert_eq!(p.recv(), Some("a"));
        assert_eq!(p.recv(), Some("c"));
        assert_eq!(p.recv(), None);
    }

    #[test]
    fn clock_skew_offsets_baseline() {
        let c = ClockSkew::new();
        assert_eq!(c.apply(100), 100);
        c.add(50);
        assert_eq!(c.apply(100), 150);
        c.add(-30);
        assert_eq!(c.apply(100), 120);
        c.set(-200);
        // Saturating: baseline 100 - offset 200 → 0.
        assert_eq!(c.apply(100), 0);
    }

    #[test]
    fn budget_is_default_constructible() {
        let b = ChaosBudget::new();
        assert!(!b.kill.armed());
        assert!(!b.partition.is_partitioned());
        assert_eq!(b.clock.offset(), 0);
    }

    #[test]
    fn kill_switch_poll_count_advances() {
        let k = KillSwitch::new();
        let _ = k.armed();
        let _ = k.armed();
        let _ = k.armed();
        assert_eq!(k.poll_count(), 3);
    }
}
