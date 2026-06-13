//! Convergence theory and the loop's stopping rule (Theorem 1).
//!
//! Under sound verification and `ρ`-productive cycles the expected epistemic
//! gap contracts geometrically (Eq. 7):
//!
//! ```text
//! E[gₜ] ≤ (1 − ρ)ᵗ g₀,
//! ```
//!
//! a Banach contraction with modulus `1 − ρ`. For any tolerance `ε ∈ (0, g₀)`
//! the cycle bound (Eq. 8) is
//!
//! ```text
//! T(ε) ≤ ⌈ ln(g₀/ε) / ln(1/(1−ρ)) ⌉ ≤ ⌈ (1/ρ) ln(g₀/ε) ⌉.
//! ```
//!
//! The RSI Loop Engine (paper §IV.B) implements this as its convergence
//! detector: it maintains an EWMA estimate `ρ̂ₜ` of the admitted-mass ratio
//! and halts when the admitted mass stays below `ε` for `k` consecutive
//! cycles — the SAHOO-calibrated stopping family.

use serde::{Deserialize, Serialize};

/// Expected gap after `t` cycles under the geometric contraction (Eq. 7):
/// `E[gₜ] = (1 − ρ)ᵗ g₀`.
///
/// `rho` is clamped into `[0, 1]` and `t` may be any cycle index.
#[must_use]
pub fn expected_gap(g0: f64, rho: f64, t: u32) -> f64 {
    let rho = rho.clamp(0.0, 1.0);
    g0 * (1.0 - rho).powi(i32::try_from(t).unwrap_or(i32::MAX))
}

/// The cycle bound `T(ε)` of Eq. (8): the number of cycles that suffice for
/// `E[g_T] ≤ ε`.
///
/// Returns `None` when the bound is undefined or infinite: `ρ ≤ 0` (no
/// contraction), `ρ ≥ 1` is treated as one-cycle convergence, or
/// `ε`/`g₀` out of the valid `0 < ε ≤ g₀` range.
#[must_use]
pub fn cycles_to_tolerance(g0: f64, eps: f64, rho: f64) -> Option<u32> {
    if !(g0 > 0.0 && eps > 0.0 && rho > 0.0) {
        return None;
    }
    if eps >= g0 {
        return Some(0);
    }
    if rho >= 1.0 {
        return Some(1);
    }
    // T = ceil( ln(g0/eps) / ln(1/(1-rho)) ).
    let numer = (g0 / eps).ln();
    let denom = (1.0 / (1.0 - rho)).ln();
    if denom <= 0.0 {
        return None;
    }
    let t = (numer / denom).ceil();
    if t.is_finite() && t >= 0.0 {
        // Safe cast: `t` is a finite non-negative ceil result.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(t as u32)
    } else {
        None
    }
}

/// Online exponentially-weighted estimate of the per-cycle productivity
/// `ρ̂ₜ` from observed admitted-mass ratios (paper §III.C " ˆρt ").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RhoEstimator {
    /// EWMA smoothing factor `α ∈ (0, 1]`; higher tracks faster.
    alpha: f64,
    /// Current estimate, `None` until the first observation.
    value: Option<f64>,
}

impl RhoEstimator {
    /// Construct with smoothing factor `alpha` (clamped into `(0, 1]`).
    #[must_use]
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha: alpha.clamp(f64::MIN_POSITIVE, 1.0),
            value: None,
        }
    }

    /// Fold in the observed admitted-mass ratio `µ(Aₜ)/µ(Gₜ)` for one cycle.
    /// The ratio is clamped into `[0, 1]`.
    pub fn observe(&mut self, admitted_ratio: f64) {
        let r = admitted_ratio.clamp(0.0, 1.0);
        self.value = Some(match self.value {
            None => r,
            Some(prev) => self.alpha.mul_add(r, (1.0 - self.alpha) * prev),
        });
    }

    /// Current estimate `ρ̂` (defaults to `0.0` before the first observation).
    #[must_use]
    pub fn rho(&self) -> f64 {
        self.value.unwrap_or(0.0)
    }

    /// Live remaining-cycle forecast `T(ε)` (Eq. 8) at the current `ρ̂` — the
    /// dashboard quantity of paper §IV.B / Appendix D.
    #[must_use]
    pub fn forecast(&self, current_gap: f64, eps: f64) -> Option<u32> {
        cycles_to_tolerance(current_gap, eps, self.rho())
    }
}

/// The patience-`k` convergence detector of Algorithm 1: declares convergence
/// once the admitted mass `µ(Aₜ)` stays below `ε` for `k` consecutive cycles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConvergenceDetector {
    /// Tolerance `ε`.
    eps: f64,
    /// Patience `k` (consecutive low-admission cycles required).
    patience: u32,
    /// Current consecutive-low-admission streak `c`.
    streak: u32,
}

impl ConvergenceDetector {
    /// Construct with tolerance `eps` and patience `k`.
    #[must_use]
    pub fn new(eps: f64, patience: u32) -> Self {
        Self {
            eps,
            patience,
            streak: 0,
        }
    }

    /// Record one cycle's admitted mass `µ(Aₜ)`. Returns `true` once the
    /// streak of below-`ε` cycles reaches the patience `k`.
    pub fn observe(&mut self, admitted_mass: f64) -> bool {
        if admitted_mass < self.eps {
            self.streak = self.streak.saturating_add(1);
        } else {
            self.streak = 0;
        }
        self.converged()
    }

    /// Whether convergence has been declared.
    #[must_use]
    pub fn converged(&self) -> bool {
        self.streak >= self.patience
    }

    /// Current consecutive-low-admission streak.
    #[must_use]
    pub const fn streak(&self) -> u32 {
        self.streak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_gap_contracts_geometrically() {
        let g0 = 1.0;
        let rho = 0.3;
        assert!((expected_gap(g0, rho, 0) - 1.0).abs() < 1e-12);
        assert!((expected_gap(g0, rho, 1) - 0.7).abs() < 1e-12);
        assert!((expected_gap(g0, rho, 2) - 0.49).abs() < 1e-12);
        // Monotone decreasing.
        assert!(expected_gap(g0, rho, 5) < expected_gap(g0, rho, 4));
    }

    #[test]
    fn cycle_bound_matches_paper_figure_1() {
        // Paper §III.C: "even modest productivity (ρ = 0.1) reaches
        // ε = 10⁻³ within T(ε) ≈ 66 cycles" (with g₀ = 1).
        let t = cycles_to_tolerance(1.0, 1e-3, 0.1).unwrap();
        assert_eq!(t, 66, "T(ε) = {t}");
    }

    #[test]
    fn cycle_bound_is_consistent_with_the_forecast() {
        // After T(ε) cycles the expected gap must be at or below ε.
        let (g0, eps, rho) = (1.0, 1e-3, 0.2);
        let t = cycles_to_tolerance(g0, eps, rho).unwrap();
        assert!(expected_gap(g0, rho, t) <= eps + 1e-12);
    }

    #[test]
    fn cycle_bound_undefined_without_contraction() {
        assert_eq!(cycles_to_tolerance(1.0, 1e-3, 0.0), None);
        assert_eq!(cycles_to_tolerance(1.0, 1e-3, -0.5), None);
    }

    #[test]
    fn already_within_tolerance_needs_zero_cycles() {
        assert_eq!(cycles_to_tolerance(1.0, 2.0, 0.3), Some(0));
    }

    #[test]
    fn rho_estimator_tracks_toward_observations() {
        let mut e = RhoEstimator::new(0.5);
        e.observe(0.4);
        assert!((e.rho() - 0.4).abs() < 1e-12);
        e.observe(0.4);
        assert!((e.rho() - 0.4).abs() < 1e-12);
        // A new lower observation pulls the estimate down.
        e.observe(0.0);
        assert!(e.rho() < 0.4);
    }

    #[test]
    fn detector_needs_patience_consecutive_low_cycles() {
        let mut d = ConvergenceDetector::new(0.01, 3);
        assert!(!d.observe(0.005)); // streak 1
        assert!(!d.observe(0.005)); // streak 2
                                    // A productive cycle resets the streak.
        assert!(!d.observe(0.5));
        assert_eq!(d.streak(), 0);
        assert!(!d.observe(0.001)); // 1
        assert!(!d.observe(0.001)); // 2
        assert!(d.observe(0.001)); // 3 => converged
    }
}
