//! Rule-driven decision table (paper §XI.B).
//!
//! A [`DecisionTable`] is an ordered list of [`Rule`]s plus a fall-through
//! [`Risk`]. The first rule whose [`Predicate`] matches the
//! [`RiskInputs`] wins; if no rule matches the table's
//! `default` outcome is returned.
//!
//! ## Monotonicity invariant
//!
//! Theorem A8 says the classifier MUST be monotone — relaxing one input
//! field can never tighten the outcome. [`verify_monotonicity`] enumerates
//! a small but representative grid of input pairs and asserts the property
//! holds; production tables should run this once at startup so a
//! misconfigured rule is caught before the kernel admits a turn.

use gauss_core::{CapToken, TaintLabel, ToolId};
use serde::{Deserialize, Serialize};

use crate::risk::{Classifier, Risk, RiskInputs};

/// Boolean predicate over [`RiskInputs`].
///
/// The variants form a small algebra so rule sets remain auditable. Add a
/// variant only after asserting the monotonicity property — every
/// predicate must be **monotone-increasing** in input strictness (more
/// risky inputs → predicate evaluates `true` at least as often).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Predicate {
    /// Always matches.
    Always,
    /// Matches when the input's cap-required contains `cap`.
    ContainsCap {
        /// The cap that must be present in the inputs.
        cap: CapToken,
    },
    /// Matches when the input's taint is at least `min` in the lattice.
    TaintAtLeast {
        /// Minimum taint level (inclusive).
        min: TaintLabel,
    },
    /// Matches when `reversible == false`.
    NonReversible,
    /// Matches a specific tool id (exact equality). Used for tool-specific
    /// overrides — typically *de-escalating* (e.g. an in-process JSON pretty-
    /// printer that's `NETWORK_POST` for OAuth reasons but actually safe).
    /// To preserve monotonicity, tool overrides MUST appear as `Auto` /
    /// `Notify` outcomes after the global-rule fall-through.
    Tool {
        /// Exact tool identifier.
        id: ToolId,
    },
    /// All sub-predicates match.
    All {
        /// Conjuncts.
        of: Vec<Predicate>,
    },
    /// At least one sub-predicate matches.
    Any {
        /// Disjuncts.
        of: Vec<Predicate>,
    },
}

impl Predicate {
    /// Evaluate the predicate against `inputs`.
    #[must_use]
    pub fn eval(&self, inputs: &RiskInputs) -> bool {
        match self {
            Self::Always => true,
            Self::ContainsCap { cap } => inputs.cap.contains(*cap),
            Self::TaintAtLeast { min } => inputs.taint >= *min,
            Self::NonReversible => !inputs.reversible,
            Self::Tool { id } => &inputs.tool == id,
            Self::All { of } => of.iter().all(|p| p.eval(inputs)),
            Self::Any { of } => of.iter().any(|p| p.eval(inputs)),
        }
    }
}

/// One rule in a [`DecisionTable`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Rule {
    /// Match predicate.
    pub predicate: Predicate,
    /// Outcome to assign when the predicate matches.
    pub outcome: Risk,
    /// Operator-readable label for diagnostics (e.g. `"crypto_signing"`).
    pub label: String,
}

impl Rule {
    /// Build a rule.
    #[must_use]
    pub fn new(predicate: Predicate, outcome: Risk, label: impl Into<String>) -> Self {
        Self {
            predicate,
            outcome,
            label: label.into(),
        }
    }
}

/// Ordered list of rules + fall-through outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DecisionTable {
    /// Rules evaluated in order; the first match wins.
    pub rules: Vec<Rule>,
    /// Fall-through outcome if no rule matches.
    pub default: Risk,
}

impl DecisionTable {
    /// Build a decision table.
    #[must_use]
    pub const fn new(rules: Vec<Rule>, default: Risk) -> Self {
        Self { rules, default }
    }

    /// Match `inputs` against the rules and return the resulting [`Risk`].
    #[must_use]
    pub fn classify(&self, inputs: &RiskInputs) -> Risk {
        for r in &self.rules {
            if r.predicate.eval(inputs) {
                return r.outcome;
            }
        }
        self.default
    }
}

impl Classifier for DecisionTable {
    fn classify(&self, inputs: &RiskInputs) -> Risk {
        Self::classify(self, inputs)
    }
}

/// Paper §XI.B canonical default table.
///
/// Order matters — the FIRST matching rule wins. Read top-to-bottom:
///
/// 1. Adversarial taint or `Deny` cap → outright `Deny`.
/// 2. `CRYPTO_SIGN` cap → `RequireApproval` regardless of reversibility.
/// 3. Non-reversible AND (`NETWORK_POST` or `SUBPROCESS_SPAWN`) →
///    `RequireApproval`.
/// 4. Non-reversible OR Web-tainted → `Notify`.
/// 5. Default → `Auto`.
#[must_use]
pub fn default_decision_table() -> DecisionTable {
    DecisionTable::new(
        vec![
            Rule::new(
                Predicate::TaintAtLeast {
                    min: TaintLabel::Adversarial,
                },
                Risk::Deny,
                "adversarial_taint_denies",
            ),
            Rule::new(
                Predicate::ContainsCap {
                    cap: CapToken::CRYPTO_SIGN,
                },
                Risk::RequireApproval,
                "crypto_sign_requires_approval",
            ),
            Rule::new(
                Predicate::All {
                    of: vec![
                        Predicate::NonReversible,
                        Predicate::Any {
                            of: vec![
                                Predicate::ContainsCap {
                                    cap: CapToken::NETWORK_POST,
                                },
                                Predicate::ContainsCap {
                                    cap: CapToken::SUBPROCESS_SPAWN,
                                },
                            ],
                        },
                    ],
                },
                Risk::RequireApproval,
                "non_reversible_high_impact",
            ),
            Rule::new(
                Predicate::Any {
                    of: vec![
                        Predicate::NonReversible,
                        Predicate::TaintAtLeast {
                            min: TaintLabel::Web,
                        },
                    ],
                },
                Risk::Notify,
                "notify_on_caution",
            ),
        ],
        Risk::Auto,
    )
}

/// Monotonicity violation reported by [`verify_monotonicity`].
#[derive(Debug, Clone, thiserror::Error)]
#[error("decision table is not monotone: relaxing inputs from {strict:?} to {lax:?} tightened the outcome from {risk_strict:?} to {risk_lax:?}")]
pub struct MonotonicityError {
    /// The stricter inputs (e.g. non-reversible).
    pub strict: RiskInputs,
    /// The more permissive inputs (e.g. reversible).
    pub lax: RiskInputs,
    /// Outcome the classifier returned for `strict`.
    pub risk_strict: Risk,
    /// Outcome the classifier returned for `lax`.
    pub risk_lax: Risk,
}

/// Verify that `table` is monotone over a canonical input grid.
///
/// For each pair `(a, b)` with `a <= b` in the per-field strictness order
/// (reversible ≤ non-reversible, lower-taint ≤ higher-taint, fewer-caps ≤
/// more-caps), we assert `classify(a) ≤ classify(b)`. Returns the first
/// violation found, or `Ok(())` if the grid is monotone.
///
/// # Errors
/// Returns [`MonotonicityError`] on the first non-monotone pair.
pub fn verify_monotonicity(table: &DecisionTable) -> Result<(), MonotonicityError> {
    // A representative cap ladder: bottom → read → write → get → post →
    // spawn → crypto → top.
    let cap_ladder = [
        CapToken::BOTTOM,
        CapToken::FILESYSTEM_READ,
        CapToken::FILESYSTEM_WRITE | CapToken::FILESYSTEM_READ,
        CapToken::NETWORK_GET | CapToken::FILESYSTEM_WRITE | CapToken::FILESYSTEM_READ,
        CapToken::NETWORK_POST
            | CapToken::NETWORK_GET
            | CapToken::FILESYSTEM_WRITE
            | CapToken::FILESYSTEM_READ,
        CapToken::SUBPROCESS_SPAWN
            | CapToken::NETWORK_POST
            | CapToken::NETWORK_GET
            | CapToken::FILESYSTEM_WRITE
            | CapToken::FILESYSTEM_READ,
        CapToken::CRYPTO_SIGN
            | CapToken::SUBPROCESS_SPAWN
            | CapToken::NETWORK_POST
            | CapToken::NETWORK_GET
            | CapToken::FILESYSTEM_WRITE
            | CapToken::FILESYSTEM_READ,
    ];
    let taints = [
        TaintLabel::Trusted,
        TaintLabel::User,
        TaintLabel::Web,
        TaintLabel::Adversarial,
    ];
    let reversibles = [true, false]; // reversible (lax) < non-reversible (strict)
    let tool = ToolId("default".into());

    // Walk only the "monotone" direction: lax indices are ≤ strict indices in
    // every field, so the strict input is at least as risky.
    for (i_lax, cap_lax) in cap_ladder.iter().enumerate() {
        for (i_strict, cap_strict) in cap_ladder.iter().enumerate().skip(i_lax) {
            for (t_lax_idx, t_lax) in taints.iter().enumerate() {
                for (t_strict_idx, t_strict) in taints.iter().enumerate().skip(t_lax_idx) {
                    for (r_lax_idx, r_lax) in reversibles.iter().enumerate() {
                        for r_strict in reversibles.iter().skip(r_lax_idx) {
                            let lax = RiskInputs::new(*cap_lax, *t_lax, *r_lax, tool.clone());
                            let strict =
                                RiskInputs::new(*cap_strict, *t_strict, *r_strict, tool.clone());
                            let risk_lax = table.classify(&lax);
                            let risk_strict = table.classify(&strict);
                            let _ = (i_strict, t_strict_idx); // silence unused warning
                            if risk_strict < risk_lax {
                                return Err(MonotonicityError {
                                    strict,
                                    lax,
                                    risk_strict,
                                    risk_lax,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_is_monotone() {
        verify_monotonicity(&default_decision_table()).unwrap();
    }

    #[test]
    fn default_table_denies_adversarial_taint() {
        let t = default_decision_table();
        let inputs = RiskInputs::new(
            CapToken::FILESYSTEM_READ,
            TaintLabel::Adversarial,
            true,
            ToolId("any".into()),
        );
        assert_eq!(t.classify(&inputs), Risk::Deny);
    }

    #[test]
    fn default_table_requires_approval_for_crypto_sign() {
        let t = default_decision_table();
        let inputs = RiskInputs::new(
            CapToken::CRYPTO_SIGN,
            TaintLabel::User,
            true,
            ToolId("signer".into()),
        );
        assert_eq!(t.classify(&inputs), Risk::RequireApproval);
    }

    #[test]
    fn default_table_requires_approval_for_non_reversible_network_post() {
        let t = default_decision_table();
        let inputs = RiskInputs::new(
            CapToken::NETWORK_POST,
            TaintLabel::User,
            /* reversible */ false,
            ToolId("send_email".into()),
        );
        assert_eq!(t.classify(&inputs), Risk::RequireApproval);
    }

    #[test]
    fn default_table_autos_trusted_reversible_filesystem_read() {
        let t = default_decision_table();
        let inputs = RiskInputs::new(
            CapToken::FILESYSTEM_READ,
            TaintLabel::Trusted,
            true,
            ToolId("read_file".into()),
        );
        assert_eq!(t.classify(&inputs), Risk::Auto);
    }

    #[test]
    fn default_table_notifies_on_web_taint() {
        let t = default_decision_table();
        let inputs = RiskInputs::new(
            CapToken::FILESYSTEM_READ,
            TaintLabel::Web,
            true,
            ToolId("scrape".into()),
        );
        assert_eq!(t.classify(&inputs), Risk::Notify);
    }

    #[test]
    fn detects_non_monotone_table() {
        // Build a deliberately broken table: more-risky inputs (Adversarial)
        // get a *less* restrictive outcome than less-risky (Trusted).
        let broken = DecisionTable::new(
            vec![
                Rule::new(
                    Predicate::TaintAtLeast {
                        min: TaintLabel::Adversarial,
                    },
                    Risk::Auto, // WRONG: adversarial should be at LEAST as strict.
                    "broken_adv_lax",
                ),
                Rule::new(
                    Predicate::TaintAtLeast {
                        min: TaintLabel::User,
                    },
                    Risk::Notify,
                    "user_notify",
                ),
            ],
            Risk::Auto,
        );
        let err = verify_monotonicity(&broken).unwrap_err();
        assert!(err.risk_strict < err.risk_lax);
    }

    #[test]
    fn rule_label_is_stable_for_default_table() {
        let t = default_decision_table();
        let labels: Vec<&str> = t.rules.iter().map(|r| r.label.as_str()).collect();
        // The default rule list is part of the operator's audit surface; a
        // refactor that renames or reorders it should require updating this
        // pin so the change is visible in code review.
        assert_eq!(
            labels,
            vec![
                "adversarial_taint_denies",
                "crypto_sign_requires_approval",
                "non_reversible_high_impact",
                "notify_on_caution",
            ]
        );
    }
}
