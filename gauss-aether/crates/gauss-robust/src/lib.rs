//! `gauss-robust` — robust declassifiers (v2 horizon, paper §XVIII.E.6).
//!
//! The Phase-1 `DefaultDeclass` and `StrictDeclass` maps are static. The
//! v2 horizon adds a **robust declassifier** that:
//!
//! 1. Wraps a base declass map.
//! 2. Tracks an adversarial-rejection counter per taint band.
//! 3. Adaptively **tightens** the map when the rejection rate exceeds
//!    a threshold — turning Web→READ into Web→BOTTOM, etc.
//!
//! The wrapper preserves antitonicity at every step (higher taint
//! always maps to ≤ caps) so `verify_antitone` continues to pass after
//! adaptation; the trust model is "monotone downgrade only" — a
//! reckless adversary causes tightening, not relaxation.

use std::sync::atomic::{AtomicU32, Ordering};

use gauss_core::{CapToken, TaintLabel};
use gauss_kernel::{default_declass, DeclassMap};
use serde::{Deserialize, Serialize};

/// Per-taint rejection counters. Each bump may tighten the underlying
/// map.
#[derive(Debug, Default)]
pub struct AdversarialRejections {
    trusted: AtomicU32,
    user: AtomicU32,
    web: AtomicU32,
    adversarial: AtomicU32,
}

impl AdversarialRejections {
    /// Snapshot the counters.
    #[must_use]
    pub fn snapshot(&self) -> [u32; 4] {
        [
            self.trusted.load(Ordering::Acquire),
            self.user.load(Ordering::Acquire),
            self.web.load(Ordering::Acquire),
            self.adversarial.load(Ordering::Acquire),
        ]
    }

    /// Bump the counter for `taint`.
    pub fn bump(&self, taint: TaintLabel) {
        let counter = self.counter_for(taint);
        counter.fetch_add(1, Ordering::AcqRel);
    }

    /// Read the counter for `taint`.
    pub fn get(&self, taint: TaintLabel) -> u32 {
        self.counter_for(taint).load(Ordering::Acquire)
    }

    const fn counter_for(&self, taint: TaintLabel) -> &AtomicU32 {
        match taint {
            TaintLabel::Trusted => &self.trusted,
            TaintLabel::User => &self.user,
            TaintLabel::Web => &self.web,
            TaintLabel::Adversarial => &self.adversarial,
        }
    }
}

/// Robust declassifier: starts at a base map, tightens monotonically as
/// adversarial-rejection counters cross thresholds.
pub struct RobustDeclass {
    /// Adaptive-tightening threshold per band (failures before
    /// tightening fires).
    pub threshold: u32,
    /// Per-band rejection counters.
    pub rejections: AdversarialRejections,
}

impl core::fmt::Debug for RobustDeclass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RobustDeclass")
            .field("threshold", &self.threshold)
            .field("rejections", &self.rejections.snapshot())
            .finish()
    }
}

impl Default for RobustDeclass {
    fn default() -> Self {
        Self::new(10)
    }
}

impl RobustDeclass {
    /// Build with a custom tightening threshold.
    #[must_use]
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold,
            rejections: AdversarialRejections::default(),
        }
    }

    /// Snapshot the current map (what `declass(taint)` returns right
    /// now).
    #[must_use]
    pub fn current(&self, taint: TaintLabel) -> CapToken {
        let base = default_declass(taint);
        let n = self.rejections.get(taint);
        if n >= self.threshold {
            // Tighten one step: clear all bits the next-stricter band
            // would clear.
            let tighter = match taint {
                TaintLabel::Trusted => default_declass(TaintLabel::User),
                TaintLabel::User => default_declass(TaintLabel::Web),
                TaintLabel::Web | TaintLabel::Adversarial => {
                    default_declass(TaintLabel::Adversarial)
                }
            };
            base.meet(tighter)
        } else {
            base
        }
    }

    /// Bump the rejection counter for `taint`.
    pub fn record_adversarial(&self, taint: TaintLabel) {
        self.rejections.bump(taint);
    }
}

impl DeclassMap for RobustDeclass {
    fn declass(&self, taint: TaintLabel) -> CapToken {
        self.current(taint)
    }
}

/// Snapshot of the declassifier's state for monitoring / debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeclassSnapshot {
    /// Threshold.
    pub threshold: u32,
    /// Per-band rejection counts (trusted/user/web/adversarial).
    pub rejections: [u32; 4],
    /// Current cap-bits per band.
    pub current_grants: [u64; 4],
}

impl DeclassSnapshot {
    /// Build from a `RobustDeclass`.
    #[must_use]
    pub fn from_declass(r: &RobustDeclass) -> Self {
        Self {
            threshold: r.threshold,
            rejections: r.rejections.snapshot(),
            current_grants: [
                r.current(TaintLabel::Trusted).bits(),
                r.current(TaintLabel::User).bits(),
                r.current(TaintLabel::Web).bits(),
                r.current(TaintLabel::Adversarial).bits(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_kernel::verify_antitone;

    #[test]
    fn baseline_matches_default_declass() {
        let r = RobustDeclass::default();
        for t in [
            TaintLabel::Trusted,
            TaintLabel::User,
            TaintLabel::Web,
            TaintLabel::Adversarial,
        ] {
            assert_eq!(r.current(t), default_declass(t));
        }
    }

    #[test]
    fn tightening_fires_after_threshold() {
        let r = RobustDeclass::new(3);
        // First 3 bumps stay below.
        for _ in 0..3 {
            r.record_adversarial(TaintLabel::Web);
        }
        // 4th observation: tightening fires (≥ threshold).
        let tightened = r.current(TaintLabel::Web);
        let baseline = default_declass(TaintLabel::Web);
        // Tightening can only remove bits.
        assert!(tightened.bits() & !baseline.bits() == 0);
        // And on Adversarial, baseline is BOTTOM; tighten == BOTTOM.
        assert_eq!(
            tightened.meet(default_declass(TaintLabel::Adversarial)),
            tightened
        );
    }

    #[test]
    fn antitone_holds_after_adversarial_storm() {
        let r = RobustDeclass::new(2);
        // Hit Trusted hard — its tightened version should still be
        // ≥ User's, which should still be ≥ Web's, etc.
        for _ in 0..10 {
            r.record_adversarial(TaintLabel::Trusted);
            r.record_adversarial(TaintLabel::User);
            r.record_adversarial(TaintLabel::Web);
        }
        // The antitone verifier from gauss-kernel must accept the
        // adapted map.
        verify_antitone(&r).unwrap();
    }

    #[test]
    fn snapshot_round_trips() {
        let r = RobustDeclass::new(5);
        r.record_adversarial(TaintLabel::Web);
        let snap = DeclassSnapshot::from_declass(&r);
        let json = serde_json::to_string(&snap).unwrap();
        let back: DeclassSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.threshold, 5);
    }

    #[test]
    fn counters_are_per_taint_band() {
        let r = RobustDeclass::default();
        r.record_adversarial(TaintLabel::Web);
        r.record_adversarial(TaintLabel::Web);
        r.record_adversarial(TaintLabel::User);
        assert_eq!(r.rejections.get(TaintLabel::Web), 2);
        assert_eq!(r.rejections.get(TaintLabel::User), 1);
        assert_eq!(r.rejections.get(TaintLabel::Adversarial), 0);
    }
}
