//! VerifierAgent — the soundness gate of Assumption 1 (paper §IV.G).
//!
//! The verifier is the admission oracle that defines `K°`. It applies tiered
//! checks, strongest first:
//!
//! * **Tier 1 (executable):** code skills run against held-out tests in a
//!   sandbox; mathematical claims are checked numerically/symbolically. The
//!   only tier that admits with no further quorum.
//! * **Tier 2 (grounded factual):** a claim must cite retrieved sources and
//!   win a *cross-family quorum* — agreement of `≥ q` experts from different
//!   providers — which reduces correlated-error admission.
//! * **Tier 3 (rubric judge):** an LLM judge for soft constraints only;
//!   Tier-3 items are admitted with capped confidence and prioritized for
//!   re-audit.
//!
//! The tier system trades the completeness `c_v` of Lemma 1 against the
//! false-admission rate `δv` of Proposition 2; Algorithm 1 tightens the active
//! tier after a drift rollback. Skill certification is statistical, so it is
//! stated in PAC form (Proposition 1, Eq. 11).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The verifier's verdict on a candidate (paper Appendix B `Verdict`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Verdict {
    /// Admitted at the given tier (1 strongest .. 3 weakest).
    Pass {
        /// Admitting tier.
        tier: u8,
    },
    /// Rejected, with a reason.
    Fail {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// Neither admitted nor rejected (e.g. a Tier-3 abstain).
    Abstain,
}

impl Verdict {
    /// Whether the verdict admits the candidate.
    #[must_use]
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }
}

/// A PAC skill-competence certificate (Proposition 1, Eq. 11).
///
/// With probability at least `1 − δ` over the test draw, the true competence
/// `p` satisfies `p ≥ p̂ − √(ln(1/δ)/(2m))`. A skill is admitted iff its lower
/// bound clears the competence threshold `τs`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PacCertificate {
    /// Empirical pass rate `p̂`.
    pub p_hat: f64,
    /// Lower confidence bound `p̂ − √(ln(1/δ)/(2m))`.
    pub ci_low: f64,
    /// Number of i.i.d. evaluation tasks `m`.
    pub m: u32,
    /// Confidence parameter `δ`.
    pub delta: f64,
}

/// Compute the Hoeffding lower confidence bound of Eq. (11).
///
/// Returns `p̂ − √(ln(1/δ)/(2m))`, or `0.0` for degenerate `m`/`δ`.
#[must_use]
pub fn pac_lower_bound(p_hat: f64, m: u32, delta: f64) -> f64 {
    if m == 0 || !(delta > 0.0 && delta < 1.0) {
        return 0.0;
    }
    let m_f = f64::from(m);
    let slack = ((1.0 / delta).ln() / (2.0 * m_f)).sqrt();
    p_hat - slack
}

/// Certify a skill from its empirical pass rate (Proposition 1).
///
/// Builds the PAC certificate and admits iff `ci_low ≥ tau_s`. Returns `None`
/// when the lower bound fails the threshold, so `S` never accumulates
/// uncertified competence.
#[must_use]
pub fn certify_skill(p_hat: f64, m: u32, delta: f64, tau_s: f64) -> Option<PacCertificate> {
    let ci_low = pac_lower_bound(p_hat, m, delta);
    if ci_low >= tau_s {
        Some(PacCertificate {
            p_hat,
            ci_low,
            m,
            delta,
        })
    } else {
        None
    }
}

/// A single expert's vote on a candidate, tagged with its provider family —
/// the input to the Tier-2 cross-family quorum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExpertVote {
    /// Provider family (e.g. `openai`, `anthropic`).
    pub family: String,
    /// Whether this expert agrees with the candidate.
    pub agrees: bool,
}

/// Tier-2 cross-family quorum (paper §IV.G): the candidate passes iff at least
/// `q` *distinct provider families* agree, reducing correlated-error
/// admission from shared pretraining corpora.
#[must_use]
pub fn cross_family_quorum(votes: &[ExpertVote], q: usize) -> bool {
    let agreeing_families: BTreeSet<&str> = votes
        .iter()
        .filter(|v| v.agrees)
        .map(|v| v.family.as_str())
        .collect();
    agreeing_families.len() >= q
}

/// The configured tier thresholds for the verifier (paper §IV.G).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VerifierConfig {
    /// Tier-2 cross-family quorum size `q`.
    pub quorum: usize,
    /// Capped confidence assigned to Tier-3 admissions.
    pub tier3_confidence_cap: f64,
}

impl Default for VerifierConfig {
    fn default() -> Self {
        Self {
            quorum: 2,
            tier3_confidence_cap: 0.5,
        }
    }
}

/// A factual-claim candidate presented to the tiered verifier.
///
/// This is a verification-input DTO: the boolean fields are independent tier
/// signals, not a state machine, so the `struct_excessive_bools` heuristic
/// does not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct ClaimCandidate {
    /// Whether the claim is Tier-1 executable (math/code numerically checked).
    pub tier1_checkable: bool,
    /// Tier-1 check outcome (only consulted when `tier1_checkable`).
    pub tier1_passes: bool,
    /// Whether the claim cites retrieved sources (Tier-2 precondition).
    pub cites_sources: bool,
    /// Cross-family votes (Tier-2 quorum input).
    pub votes: Vec<ExpertVote>,
    /// Whether a Tier-3 rubric judge approves (soft constraints only).
    pub tier3_judge_approves: bool,
    /// Whether the derivation touches an evaluation probe — the
    /// anti-contamination check that rejects outright (paper §VI, [7]).
    pub touches_probe: bool,
}

/// Apply the tiered verification of paper §IV.G to a factual claim, strongest
/// tier first. Returns the [`Verdict`].
///
/// Anti-contamination is checked before any tier: a derivation that touches an
/// evaluation probe fails outright regardless of tier (provenance/anti-cheat
/// judge, paper §VI).
#[must_use]
pub fn verify_claim(candidate: &ClaimCandidate, cfg: &VerifierConfig) -> Verdict {
    if candidate.touches_probe {
        return Verdict::Fail {
            reason: "derivation touches an evaluation probe".to_owned(),
        };
    }
    // Tier 1: executable / numerically checkable — admits with no quorum.
    if candidate.tier1_checkable {
        return if candidate.tier1_passes {
            Verdict::Pass { tier: 1 }
        } else {
            Verdict::Fail {
                reason: "tier-1 executable check failed".to_owned(),
            }
        };
    }
    // Tier 2: grounded factual — must cite sources and win a cross-family quorum.
    if candidate.cites_sources && cross_family_quorum(&candidate.votes, cfg.quorum) {
        return Verdict::Pass { tier: 2 };
    }
    // Tier 3: rubric judge for soft constraints — capped confidence, re-audited.
    if candidate.tier3_judge_approves {
        return Verdict::Pass { tier: 3 };
    }
    Verdict::Abstain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn votes(families: &[(&str, bool)]) -> Vec<ExpertVote> {
        families
            .iter()
            .map(|&(f, a)| ExpertVote {
                family: f.to_owned(),
                agrees: a,
            })
            .collect()
    }

    #[test]
    fn pac_bound_is_below_the_empirical_rate() {
        let lb = pac_lower_bound(0.9, 100, 0.05);
        assert!(lb < 0.9 && lb > 0.0, "lb = {lb}");
    }

    #[test]
    fn pac_bound_tightens_with_more_tests() {
        let few = pac_lower_bound(0.9, 10, 0.05);
        let many = pac_lower_bound(0.9, 1000, 0.05);
        assert!(many > few, "more tests should raise the lower bound");
    }

    #[test]
    fn skill_certified_only_above_threshold() {
        // High pass rate, many tests => certified above a modest threshold.
        assert!(certify_skill(0.95, 500, 0.05, 0.8).is_some());
        // Same rate but a high threshold => rejected.
        assert!(certify_skill(0.95, 500, 0.05, 0.99).is_none());
        // Too few tests => loose bound => rejected.
        assert!(certify_skill(0.95, 3, 0.05, 0.8).is_none());
    }

    #[test]
    fn quorum_counts_distinct_families_only() {
        // Two agreeing votes but same family => quorum of 2 not met.
        assert!(!cross_family_quorum(&votes(&[("openai", true), ("openai", true)]), 2));
        // Two distinct families agree => met.
        assert!(cross_family_quorum(&votes(&[("openai", true), ("anthropic", true)]), 2));
    }

    #[test]
    fn tier1_admits_executable_claims_without_quorum() {
        let c = ClaimCandidate {
            tier1_checkable: true,
            tier1_passes: true,
            cites_sources: false,
            votes: Vec::new(),
            tier3_judge_approves: false,
            touches_probe: false,
        };
        assert_eq!(verify_claim(&c, &VerifierConfig::default()), Verdict::Pass { tier: 1 });
    }

    #[test]
    fn tier2_requires_sources_and_cross_family_quorum() {
        let c = ClaimCandidate {
            tier1_checkable: false,
            tier1_passes: false,
            cites_sources: true,
            votes: votes(&[("openai", true), ("google", true)]),
            tier3_judge_approves: false,
            touches_probe: false,
        };
        assert_eq!(verify_claim(&c, &VerifierConfig::default()), Verdict::Pass { tier: 2 });
    }

    #[test]
    fn anti_contamination_rejects_probe_touching_derivations() {
        let c = ClaimCandidate {
            tier1_checkable: true,
            tier1_passes: true,
            cites_sources: true,
            votes: votes(&[("openai", true), ("anthropic", true)]),
            tier3_judge_approves: true,
            touches_probe: true,
        };
        // Even though every tier would pass, the probe touch fails it outright.
        assert!(matches!(verify_claim(&c, &VerifierConfig::default()), Verdict::Fail { .. }));
    }

    #[test]
    fn tier3_abstains_without_judge_approval() {
        let c = ClaimCandidate {
            tier1_checkable: false,
            tier1_passes: false,
            cites_sources: false,
            votes: Vec::new(),
            tier3_judge_approves: false,
            touches_probe: false,
        };
        assert_eq!(verify_claim(&c, &VerifierConfig::default()), Verdict::Abstain);
    }
}
