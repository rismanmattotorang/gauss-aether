//! `gauss-learnt` — learnt risk classifier `Φ̂` (v2 horizon, paper
//! §XVIII.E.4).
//!
//! The Phase-7 [`gauss_sag::DecisionTable`] is rule-driven and
//! auditable line-by-line. The v2 horizon adds a **learnt scorer** that
//! wraps the rule table:
//!
//! 1. Run the rule table to get the **baseline band**.
//! 2. Compute a logistic-regression score over per-input features.
//! 3. Take the **strictest** of the two bands (`Risk::join` — never
//!    relax the rule outcome, only tighten it).
//!
//! This preserves the SPECS monotonicity guarantee: the rule table is
//! the floor; the scorer can only escalate. Models that wanted to
//! *de-escalate* would defeat the SAG audit — they're rejected by
//! construction here.
//!
//! Production deployments train the logistic weights offline against
//! labelled prior-decision data; this crate ships the trait surface +
//! a deterministic linear-combination scorer with fixed weights.

use gauss_core::{CapToken, TaintLabel};
use gauss_sag::{Classifier, Risk, RiskInputs};
use serde::{Deserialize, Serialize};

/// Logistic-regression scorer over four hand-engineered features.
///
/// Features:
///
/// 0. `cap_depth` — `popcount(cap_required) / 64` ∈ `[0, 1]`.
/// 1. `taint_band` — `TaintLabel::Trusted..Adversarial` mapped to
///    `0..3` then normalised to `[0, 1]`.
/// 2. `non_reversible` — `1.0` if not reversible, else `0.0`.
/// 3. `crypto_or_subprocess` — `1.0` if the cap requires `CRYPTO_SIGN`
///    or `SUBPROCESS_SPAWN`, else `0.0`.
///
/// The score is `σ(w · features + b)` where `σ(z) = 1 / (1 + e^{-z})`.
/// The score thresholds the band:
///
/// * `score < 0.25` → `Risk::Auto`
/// * `score < 0.50` → `Risk::Notify`
/// * `score < 0.85` → `Risk::RequireApproval`
/// * else            → `Risk::Deny`
///
/// The classifier returns `learnt.join(rule_table)` so the rule table
/// is always a floor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LogisticScorer {
    /// Bias term.
    pub bias: f64,
    /// 4-vector of feature weights (`cap_depth`, taint, `non_reversible`,
    /// `crypto_or_subprocess`).
    pub weights: [f64; 4],
    /// Decision thresholds (4 values, monotone increasing).
    pub thresholds: [f64; 4],
}

impl Default for LogisticScorer {
    fn default() -> Self {
        // Hand-tuned weights that approximate the paper §XI.B intent:
        // higher cap depth + higher taint + non-reversibility + crypto
        // all increase the score; the four bands are spaced
        // sigmoid-friendly.
        Self {
            bias: -2.0,
            weights: [3.0, 4.0, 1.5, 5.0],
            thresholds: [0.25, 0.50, 0.85, 1.10], // last is a sentinel
        }
    }
}

impl LogisticScorer {
    /// Build with custom weights + thresholds.
    #[must_use]
    pub const fn new(bias: f64, weights: [f64; 4], thresholds: [f64; 4]) -> Self {
        Self {
            bias,
            weights,
            thresholds,
        }
    }

    /// Compute the score for `inputs` (no clamping).
    #[must_use]
    pub fn score(&self, inputs: &RiskInputs) -> f64 {
        let features = features_of(inputs);
        // Use `f64::mul_add` per clippy `suboptimal_flops` — fused
        // multiply-add is the recommended fp-precision-preserving form.
        let z = self.weights[0].mul_add(
            features[0],
            self.weights[1].mul_add(
                features[1],
                self.weights[2]
                    .mul_add(features[2], self.weights[3].mul_add(features[3], self.bias)),
            ),
        );
        sigmoid(z)
    }

    /// Band the score into one of four risks.
    #[must_use]
    pub fn band(&self, score: f64) -> Risk {
        if score < self.thresholds[0] {
            Risk::Auto
        } else if score < self.thresholds[1] {
            Risk::Notify
        } else if score < self.thresholds[2] {
            Risk::RequireApproval
        } else {
            Risk::Deny
        }
    }
}

impl Classifier for LogisticScorer {
    fn classify(&self, inputs: &RiskInputs) -> Risk {
        self.band(self.score(inputs))
    }
}

/// Composite classifier: floor-by-table, ceiling-by-scorer.
///
/// `combined.classify(inputs) = table.classify(inputs).join(scorer.classify(inputs))`.
///
/// The composite is monotone iff both inputs are monotone; the SAG
/// rule table is monotone by `gauss-sag::verify_monotonicity`, and the
/// logistic scorer is monotone iff its weights are non-negative — the
/// default weights satisfy this.
pub struct LearntClassifier<T: Classifier, S: Classifier> {
    /// The rule-driven floor.
    pub table: T,
    /// The learnt scorer.
    pub scorer: S,
}

impl<T: Classifier, S: Classifier> LearntClassifier<T, S> {
    /// Build.
    pub const fn new(table: T, scorer: S) -> Self {
        Self { table, scorer }
    }
}

impl<T: Classifier, S: Classifier> Classifier for LearntClassifier<T, S> {
    fn classify(&self, inputs: &RiskInputs) -> Risk {
        let by_table = self.table.classify(inputs);
        let by_scorer = self.scorer.classify(inputs);
        by_table.join(by_scorer)
    }
}

fn features_of(inputs: &RiskInputs) -> [f64; 4] {
    #[allow(clippy::cast_precision_loss)]
    let cap_depth = f64::from(inputs.cap.bits().count_ones()) / 64.0;
    let taint = match inputs.taint {
        TaintLabel::Trusted => 0.0,
        TaintLabel::User => 1.0 / 3.0,
        TaintLabel::Web => 2.0 / 3.0,
        TaintLabel::Adversarial => 1.0,
    };
    let non_reversible = if inputs.reversible { 0.0 } else { 1.0 };
    let crypto_or_subprocess = if inputs.cap.contains(CapToken::CRYPTO_SIGN)
        || inputs.cap.contains(CapToken::SUBPROCESS_SPAWN)
    {
        1.0
    } else {
        0.0
    };
    [cap_depth, taint, non_reversible, crypto_or_subprocess]
}

#[inline]
fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;
    use gauss_sag::default_decision_table;

    fn inputs(cap: CapToken, taint: TaintLabel, reversible: bool) -> RiskInputs {
        RiskInputs::new(cap, taint, reversible, ToolId("t".into()))
    }

    #[test]
    fn scorer_returns_auto_for_safe_inputs() {
        let s = LogisticScorer::default();
        let i = inputs(CapToken::FILESYSTEM_READ, TaintLabel::Trusted, true);
        assert_eq!(s.classify(&i), Risk::Auto);
    }

    #[test]
    fn scorer_escalates_for_high_risk_inputs() {
        let s = LogisticScorer::default();
        let i = inputs(CapToken::CRYPTO_SIGN, TaintLabel::Adversarial, false);
        assert_eq!(s.classify(&i), Risk::Deny);
    }

    #[test]
    fn composite_floor_holds_at_adversarial_taint() {
        // Even if the scorer somehow returns Auto, the rule table denies
        // adversarial taint outright.
        struct AlwaysAuto;
        impl Classifier for AlwaysAuto {
            fn classify(&self, _: &RiskInputs) -> Risk {
                Risk::Auto
            }
        }
        let c = LearntClassifier::new(default_decision_table(), AlwaysAuto);
        let i = inputs(CapToken::FILESYSTEM_READ, TaintLabel::Adversarial, true);
        assert_eq!(c.classify(&i), Risk::Deny);
    }

    #[test]
    fn composite_ceiling_tightens_above_table() {
        // Rule table says Auto; scorer escalates to RequireApproval.
        struct AlwaysApproval;
        impl Classifier for AlwaysApproval {
            fn classify(&self, _: &RiskInputs) -> Risk {
                Risk::RequireApproval
            }
        }
        let c = LearntClassifier::new(default_decision_table(), AlwaysApproval);
        let i = inputs(CapToken::FILESYSTEM_READ, TaintLabel::Trusted, true);
        // Table: Auto. Scorer: RequireApproval. Join: RequireApproval.
        assert_eq!(c.classify(&i), Risk::RequireApproval);
    }

    #[test]
    fn score_is_in_unit_interval() {
        let s = LogisticScorer::default();
        for cap in [
            CapToken::BOTTOM,
            CapToken::FILESYSTEM_READ,
            CapToken::NETWORK_POST,
            CapToken::CRYPTO_SIGN,
        ] {
            for taint in [
                TaintLabel::Trusted,
                TaintLabel::User,
                TaintLabel::Web,
                TaintLabel::Adversarial,
            ] {
                for reversible in [true, false] {
                    let v = s.score(&inputs(cap, taint, reversible));
                    assert!((0.0..=1.0).contains(&v), "score {v} out of [0,1]");
                }
            }
        }
    }

    #[test]
    fn round_trip_through_serde() {
        let s = LogisticScorer::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: LogisticScorer = serde_json::from_str(&json).unwrap();
        for i in 0..4 {
            assert!((back.weights[i] - s.weights[i]).abs() < 1e-12);
        }
    }
}
