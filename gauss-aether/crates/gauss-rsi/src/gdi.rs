//! The SAHOO Goal Drift Index and its hard gate (Eq. 17, Proposition 3).
//!
//! The safety gate inside Φ uses the SAHOO drift index: semantic, lexical,
//! structural, and distributional drift components measured against the
//! checkpointed baseline, combined under calibrated weights summing to one:
//!
//! ```text
//! GDIₜ = w_s·Δ_sem + w_ℓ·Δ_lex + w_st·Δ_str + w_d·Δ_dist.
//! ```
//!
//! When `GDIₜ > τ` (or a critical constraint is violated, `CPS < 1`) the
//! engine rolls back to the last checkpoint — a `gauss-checkpoint` snapshot
//! restore by construction of the knowledge-space design.

use serde::{Deserialize, Serialize};

/// Calibrated drift weights `w•`, normalised to sum to one (paper §IV.F,
/// "calibrated on a small labeled set exactly as in SAHOO").
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DriftWeights {
    /// Semantic-drift weight `w_s`.
    pub sem: f64,
    /// Lexical-drift weight `w_ℓ`.
    pub lex: f64,
    /// Structural-drift weight `w_st`.
    pub str_: f64,
    /// Distributional-drift weight `w_d`.
    pub dist: f64,
}

impl Default for DriftWeights {
    fn default() -> Self {
        // Equal weighting until calibration data is available.
        Self {
            sem: 0.25,
            lex: 0.25,
            str_: 0.25,
            dist: 0.25,
        }
    }
}

impl DriftWeights {
    /// Construct and renormalise so the four weights sum to one. If every
    /// weight is zero (or negative) falls back to the uniform default.
    #[must_use]
    pub fn new(sem: f64, lex: f64, str_: f64, dist: f64) -> Self {
        let sem = sem.max(0.0);
        let lex = lex.max(0.0);
        let str_ = str_.max(0.0);
        let dist = dist.max(0.0);
        let total = sem + lex + str_ + dist;
        if total <= 0.0 {
            return Self::default();
        }
        Self {
            sem: sem / total,
            lex: lex / total,
            str_: str_ / total,
            dist: dist / total,
        }
    }
}

/// The four measured drift components for one cycle.
///
/// Each is a non-negative distance against the checkpoint baseline, mapping to
/// the CriticAgent's embedding cosine distance, Jensen–Shannon vocabulary
/// divergence, structural-feature deltas, and response-embedding Wasserstein
/// distance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DriftComponents {
    /// `Δ_sem` — semantic drift (embedding cosine distance).
    pub sem: f64,
    /// `Δ_lex` — lexical drift (Jensen–Shannon vocabulary divergence).
    pub lex: f64,
    /// `Δ_str` — structural drift (structural-feature delta).
    pub str_: f64,
    /// `Δ_dist` — distributional drift (Wasserstein on response embeddings).
    pub dist: f64,
}

impl DriftComponents {
    /// Construct from the four components.
    #[must_use]
    pub const fn new(sem: f64, lex: f64, str_: f64, dist: f64) -> Self {
        Self {
            sem,
            lex,
            str_,
            dist,
        }
    }

    /// The Goal Drift Index `GDIₜ` of Eq. (17) under the supplied weights.
    #[must_use]
    pub fn gdi(&self, w: &DriftWeights) -> f64 {
        w.sem.mul_add(
            self.sem,
            w.lex
                .mul_add(self.lex, w.str_.mul_add(self.str_, w.dist * self.dist)),
        )
    }
}

/// The drift gate's verdict for one cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DriftVerdict {
    /// `GDIₜ ≤ τ` and all critical constraints hold — proceed.
    Proceed,
    /// `GDIₜ > τ` or a critical constraint is violated — roll back to the
    /// last checkpoint and tighten the verifier tier (Algorithm 1).
    Rollback,
}

/// The SAHOO drift gate: combines the GDI threshold `τ` with the hard
/// constraint-preservation check `CPS = 1` (paper §IV.F / Algorithm 1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DriftGate {
    /// Calibrated weights.
    pub weights: DriftWeights,
    /// Drift threshold `τ`.
    pub tau: f64,
}

impl DriftGate {
    /// Construct with weights and threshold `tau`.
    #[must_use]
    pub const fn new(weights: DriftWeights, tau: f64) -> Self {
        Self { weights, tau }
    }

    /// Evaluate the gate. `critical_constraints_hold` is the `CPS = 1` flag:
    /// if any critical constraint is violated the gate rolls back regardless
    /// of the GDI value.
    #[must_use]
    pub fn evaluate(
        &self,
        drift: &DriftComponents,
        critical_constraints_hold: bool,
    ) -> DriftVerdict {
        if !critical_constraints_hold || drift.gdi(&self.weights) > self.tau {
            DriftVerdict::Rollback
        } else {
            DriftVerdict::Proceed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_renormalise_to_one() {
        let w = DriftWeights::new(1.0, 1.0, 1.0, 1.0);
        let sum = w.sem + w.lex + w.str_ + w.dist;
        assert!((sum - 1.0).abs() < 1e-12);
        assert!((w.sem - 0.25).abs() < 1e-12);
    }

    #[test]
    fn degenerate_weights_fall_back_to_uniform() {
        let w = DriftWeights::new(0.0, 0.0, 0.0, 0.0);
        assert_eq!(w, DriftWeights::default());
    }

    #[test]
    fn gdi_is_the_weighted_sum() {
        let w = DriftWeights::new(0.4, 0.3, 0.2, 0.1);
        let d = DriftComponents::new(1.0, 1.0, 1.0, 1.0);
        // Weighted sum of all-ones equals the sum of (normalised) weights = 1.
        assert!((d.gdi(&w) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn gate_proceeds_below_threshold() {
        let gate = DriftGate::new(DriftWeights::default(), 0.5);
        let drift = DriftComponents::new(0.1, 0.1, 0.1, 0.1);
        assert_eq!(gate.evaluate(&drift, true), DriftVerdict::Proceed);
    }

    #[test]
    fn gate_rolls_back_above_threshold() {
        let gate = DriftGate::new(DriftWeights::default(), 0.2);
        let drift = DriftComponents::new(0.8, 0.8, 0.8, 0.8);
        assert_eq!(gate.evaluate(&drift, true), DriftVerdict::Rollback);
    }

    #[test]
    fn critical_constraint_violation_forces_rollback() {
        // CPS < 1 rolls back even with zero drift.
        let gate = DriftGate::new(DriftWeights::default(), 1.0);
        let drift = DriftComponents::default();
        assert_eq!(gate.evaluate(&drift, false), DriftVerdict::Rollback);
    }
}
