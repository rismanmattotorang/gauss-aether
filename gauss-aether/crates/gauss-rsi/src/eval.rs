//! Pre-registered evaluation harness (paper §VI).
//!
//! The paper specifies a budget-matched protocol that states *in advance* what
//! confirms or falsifies hypothesis H. This module ships the metric and
//! protocol logic — the `ΔK/ΔS` knowledge/skill-delta metrics, the systems
//! under test, the ablation switches, and the decision rule for H — as
//! deterministic, testable functions. The live benchmark drivers (MMLU, ARC,
//! MATH-500, GSM8K) plug into `gauss-bench`.
//!
//! `ΔK` counts independently audited verified claims held by a system minus
//! those elicitable from the budget-matched union of single models; the
//! *synergy* count restricts `ΔK` to items whose provenance derivation spans
//! at least two model families (a direct estimate of `µ(Σ ∩ K_T)`,
//! Theorem 2). `ΔS` is the analogous count over PAC-certified skill families.

use serde::{Deserialize, Serialize};

/// The systems under test (paper §VI.B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SystemUnderTest {
    /// B1 — each single model alone (zero-shot / CoT) at full budget.
    SingleModel,
    /// B2 — naive ensemble (majority vote / best-of-n) at matched budget.
    NaiveEnsemble,
    /// B3 — routed but memoryless (`K = S = ∅` throughout): isolates `ΔR`.
    RoutedMemoryless,
    /// B4 — full Gauss-Agent0.
    GaussAgent0,
}

/// Ablation switches (paper §VI.B), each targeting one factor of Lemma 1.
///
/// These are independent on/off experimental toggles, not a state machine, so
/// the `struct_excessive_bools` heuristic does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct Ablations {
    /// Drop the DualRAG graph path (targets `r_L`).
    pub no_graph: bool,
    /// Drop the DualRAG vector path (targets `r_L`).
    pub no_vector: bool,
    /// Log-only admission (targets Assumption 1; inflates contamination).
    pub no_verifier: bool,
    /// Random curriculum (targets coverage `β`).
    pub no_critic: bool,
    /// Uniform routing instead of learned (targets `εₓ` vs. learned routing).
    pub uniform_router: bool,
}

impl Ablations {
    /// The unablated full system (B4).
    #[must_use]
    pub const fn none() -> Self {
        Self {
            no_graph: false,
            no_vector: false,
            no_verifier: false,
            no_critic: false,
            uniform_router: false,
        }
    }
}

/// The `ΔK/ΔS` knowledge/skill-delta metric (paper §VI.A).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct KnowledgeSkillDelta {
    /// `ΔK` — audited verified claims held minus the budget-matched union of
    /// single models.
    pub delta_k: i64,
    /// Synergy count — `ΔK` restricted to multi-family derivations
    /// (`µ̂(Σ ∩ K_T)`).
    pub synergy: i64,
    /// `ΔS` — PAC-certified skill families minus the single-model union.
    pub delta_s: i64,
}

/// Compute the `ΔK/ΔS` metric from audited counts.
///
/// `system_claims`/`system_skills` are the audited verified counts held by the
/// system; `union_claims`/`union_skills` are the budget-matched single-model
/// union counts; `synergy_claims` is the count whose derivation spans `≥ 2`
/// model families.
#[must_use]
pub fn knowledge_skill_delta(
    system_claims: u64,
    union_claims: u64,
    synergy_claims: u64,
    system_skills: u64,
    union_skills: u64,
) -> KnowledgeSkillDelta {
    let to_i = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    KnowledgeSkillDelta {
        delta_k: to_i(system_claims).saturating_sub(to_i(union_claims)),
        synergy: to_i(synergy_claims),
        delta_s: to_i(system_skills).saturating_sub(to_i(union_skills)),
    }
}

/// Per-system benchmark accuracy on the four public benchmarks (paper
/// Table IV), each in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BenchmarkScores {
    /// MMLU accuracy.
    pub mmlu: f64,
    /// ARC-Challenge accuracy.
    pub arc: f64,
    /// MATH-500 exact match.
    pub math: f64,
    /// GSM8K exact match.
    pub gsm8k: f64,
}

impl BenchmarkScores {
    /// The four scores as an array, in Table IV order.
    #[must_use]
    pub const fn as_array(&self) -> [f64; 4] {
        [self.mmlu, self.arc, self.math, self.gsm8k]
    }

    /// Count of benchmarks on which `self` strictly exceeds `other`.
    #[must_use]
    pub fn benchmarks_beating(&self, other: &Self) -> usize {
        self.as_array()
            .iter()
            .zip(other.as_array().iter())
            .filter(|(a, b)| a > b)
            .count()
    }
}

/// The decision rule for hypothesis H (paper §VI.C).
///
/// H is **supported** iff B4 (full Gauss-Agent0) exceeds *both* the best
/// single model (B1) and the naive ensemble (B2) on at least three of four
/// benchmarks at matched budget, **and** the audited synergy count is positive
/// with a nonzero lower confidence bound. H is **falsified** if B4 fails to
/// beat B2 anywhere, or if the synergy count is statistically zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HypothesisOutcome {
    /// H supported by this run.
    Supported,
    /// H falsified by this run.
    Falsified,
    /// Neither threshold met — inconclusive.
    Inconclusive,
}

/// Evaluate hypothesis H from the run's measured quantities.
///
/// `synergy_ci_low` is the lower confidence bound on the audited synergy
/// count; H requires it strictly positive.
#[must_use]
pub fn evaluate_hypothesis(
    b4: &BenchmarkScores,
    b1_best: &BenchmarkScores,
    b2_ensemble: &BenchmarkScores,
    synergy_ci_low: f64,
) -> HypothesisOutcome {
    let beats_b2_anywhere = b4.benchmarks_beating(b2_ensemble) > 0;
    if !beats_b2_anywhere || synergy_ci_low <= 0.0 {
        return HypothesisOutcome::Falsified;
    }
    let beats_b1 = b4.benchmarks_beating(b1_best) >= 3;
    let beats_b2 = b4.benchmarks_beating(b2_ensemble) >= 3;
    if beats_b1 && beats_b2 && synergy_ci_low > 0.0 {
        HypothesisOutcome::Supported
    } else {
        HypothesisOutcome::Inconclusive
    }
}

/// Theory-grounded telemetry reported alongside accuracy (paper §VI.A): the
/// monitored quantities that discharge each assumption.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Telemetry {
    /// `ρ̂` — admitted-mass ratio (productivity, Theorem 1).
    pub rho_hat: f64,
    /// `r̂_L` — premise recall (Lemma 1).
    pub premise_recall: f64,
    /// `Δ̂R` — routing advantage (Theorem 3).
    pub routing_advantage: f64,
    /// Mean GDI over the run (SAHOO drift).
    pub mean_gdi: f64,
    /// Number of rollbacks triggered.
    pub rollbacks: u32,
    /// Capability–alignment ratio (SAHOO lens, ref. 8).
    pub capability_alignment_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_k_is_system_minus_union() {
        let d = knowledge_skill_delta(120, 90, 30, 10, 6);
        assert_eq!(d.delta_k, 30);
        assert_eq!(d.synergy, 30);
        assert_eq!(d.delta_s, 4);
    }

    #[test]
    fn delta_k_can_be_negative() {
        let d = knowledge_skill_delta(50, 90, 0, 0, 0);
        assert_eq!(d.delta_k, -40);
    }

    #[test]
    fn benchmarks_beating_counts_strict_wins() {
        let a = BenchmarkScores {
            mmlu: 0.8,
            arc: 0.7,
            math: 0.6,
            gsm8k: 0.9,
        };
        let b = BenchmarkScores {
            mmlu: 0.7,
            arc: 0.7,
            math: 0.5,
            gsm8k: 0.8,
        };
        // mmlu, math, gsm8k strictly beat; arc ties.
        assert_eq!(a.benchmarks_beating(&b), 3);
    }

    #[test]
    fn hypothesis_supported_when_b4_dominates_with_synergy() {
        let b4 = BenchmarkScores {
            mmlu: 0.85,
            arc: 0.78,
            math: 0.65,
            gsm8k: 0.92,
        };
        let weaker = BenchmarkScores {
            mmlu: 0.80,
            arc: 0.70,
            math: 0.60,
            gsm8k: 0.88,
        };
        assert_eq!(
            evaluate_hypothesis(&b4, &weaker, &weaker, 5.0),
            HypothesisOutcome::Supported
        );
    }

    #[test]
    fn hypothesis_falsified_when_synergy_is_zero() {
        let b4 = BenchmarkScores {
            mmlu: 0.85,
            arc: 0.78,
            math: 0.65,
            gsm8k: 0.92,
        };
        let weaker = BenchmarkScores {
            mmlu: 0.80,
            arc: 0.70,
            math: 0.60,
            gsm8k: 0.88,
        };
        // Synergy count statistically zero => falsified, as §VI.C prescribes.
        assert_eq!(
            evaluate_hypothesis(&b4, &weaker, &weaker, 0.0),
            HypothesisOutcome::Falsified
        );
    }

    #[test]
    fn hypothesis_falsified_when_b4_never_beats_ensemble() {
        let b4 = BenchmarkScores {
            mmlu: 0.70,
            arc: 0.70,
            math: 0.60,
            gsm8k: 0.80,
        };
        let ensemble = BenchmarkScores {
            mmlu: 0.80,
            arc: 0.75,
            math: 0.65,
            gsm8k: 0.85,
        };
        assert_eq!(
            evaluate_hypothesis(&b4, &ensemble, &ensemble, 5.0),
            HypothesisOutcome::Falsified
        );
    }
}
