//! Per-cycle productivity `ρ` and its factorization (Lemma 1).
//!
//! Assumption 3 (paper §III.B) requires a productivity floor: whenever the
//! gap is non-empty, the expected admitted mass is at least `ρ·µ(Gₜ)` for
//! some `ρ ∈ (0, 1]`. Lemma 1 *factorizes* that floor into five
//! component-level probabilities the architecture can monitor independently:
//!
//! ```text
//! ρ ≥ β · εₓ · r_L · p_g · c_v > 0,
//! ```
//!
//! where `β` is curriculum coverage, `εₓ` the router exploration floor,
//! `r_L` DualRAG premise recall, `p_g` the expert emission probability, and
//! `c_v` verifier completeness. The diagnostic value (paper §VII.A) is that
//! when progress stalls, exactly one factor has collapsed — and each is
//! separately instrumented.

use serde::{Deserialize, Serialize};

/// The five conditional probabilities of Lemma 1. Each is a probability in
/// `[0, 1]`; the productivity floor is their product.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProductivityFactors {
    /// `β` — curriculum coverage: probability the cycle targets a given gap
    /// item (CriticAgent frontier filter).
    pub coverage: f64,
    /// `εₓ` — router exploration floor: probability every expert is selected
    /// (QueryRouter, Algorithm 3).
    pub exploration: f64,
    /// `r_L` — DualRAG premise recall: probability a targeted derivable item's
    /// premise set lies within the beam-`b`, depth-`L` neighbourhood and
    /// survives fusion.
    pub premise_recall: f64,
    /// `p_g` — expert emission probability (Definition 1).
    pub emission: f64,
    /// `c_v` — verifier completeness: probability a true emitted item is
    /// certified.
    pub verifier_completeness: f64,
}

impl ProductivityFactors {
    /// Construct from the five factors. Each is clamped into `[0, 1]` so a
    /// mis-instrumented signal can never push the floor outside its valid
    /// range.
    #[must_use]
    pub fn new(
        coverage: f64,
        exploration: f64,
        premise_recall: f64,
        emission: f64,
        verifier_completeness: f64,
    ) -> Self {
        Self {
            coverage: coverage.clamp(0.0, 1.0),
            exploration: exploration.clamp(0.0, 1.0),
            premise_recall: premise_recall.clamp(0.0, 1.0),
            emission: emission.clamp(0.0, 1.0),
            verifier_completeness: verifier_completeness.clamp(0.0, 1.0),
        }
    }

    /// The productivity lower bound `ρ ≥ β·εₓ·r_L·p_g·c_v` (Eq. 6).
    #[must_use]
    pub fn lower_bound(&self) -> f64 {
        self.coverage
            * self.exploration
            * self.premise_recall
            * self.emission
            * self.verifier_completeness
    }

    /// The factor that has collapsed furthest — the diagnostic of §VII.A.
    /// Returns the `(name, value)` of the smallest factor, so an operator can
    /// see *where* a stalled loop lost its productivity.
    #[must_use]
    pub fn weakest_factor(&self) -> (&'static str, f64) {
        let factors = [
            ("coverage (β)", self.coverage),
            ("exploration (εₓ)", self.exploration),
            ("premise_recall (r_L)", self.premise_recall),
            ("emission (p_g)", self.emission),
            ("verifier_completeness (c_v)", self.verifier_completeness),
        ];
        let mut weakest = factors[0];
        for f in factors {
            if f.1 < weakest.1 {
                weakest = f;
            }
        }
        weakest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_bound_is_the_product() {
        let f = ProductivityFactors::new(0.5, 0.5, 0.5, 0.5, 0.5);
        assert!((f.lower_bound() - 0.03125).abs() < 1e-12);
    }

    #[test]
    fn lower_bound_is_positive_when_every_factor_is() {
        let f = ProductivityFactors::new(0.8, 0.05, 0.6, 0.7, 0.9);
        assert!(f.lower_bound() > 0.0);
    }

    #[test]
    fn a_single_zero_factor_kills_productivity() {
        // NOGRAPH ablation: r_L collapses => ρ floor is zero.
        let f = ProductivityFactors::new(0.8, 0.05, 0.0, 0.7, 0.9);
        assert!((f.lower_bound()).abs() < 1e-12);
    }

    #[test]
    fn factors_are_clamped_into_unit_interval() {
        let f = ProductivityFactors::new(2.0, -1.0, 0.5, 0.5, 0.5);
        assert!((f.coverage - 1.0).abs() < 1e-12);
        assert!((f.exploration).abs() < 1e-12);
    }

    #[test]
    fn weakest_factor_localizes_the_stall() {
        let f = ProductivityFactors::new(0.9, 0.8, 0.01, 0.7, 0.95);
        let (name, value) = f.weakest_factor();
        assert_eq!(name, "premise_recall (r_L)");
        assert!((value - 0.01).abs() < 1e-12);
    }
}
