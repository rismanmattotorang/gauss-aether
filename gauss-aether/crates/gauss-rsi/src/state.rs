//! The RSI state `x = (K, S)` and the improvement operator Φ.
//!
//! Paper §III.A, Eqs. (1)–(2). The system state is a pair of finite sets —
//! verified knowledge `K` and certified skills `S` — drawn from countable
//! universes `K°`, `S°`. The metric space `(X, d)` of Eq. (1) is
//!
//! ```text
//! d(x, x′) = µ(K △ K′) + ν(S △ S′),
//! ```
//!
//! where `△` is symmetric difference and `µ`, `ν` are finite importance
//! measures (uniform counting by default; see [`Measure`]). One cycle of the
//! loop is the operator Φ of Eq. (2):
//!
//! ```text
//! Φ(xₜ) = ( (Kₜ ∪ Aᴷ) \ Fᴷ , (Sₜ ∪ Aˢ) \ Fˢ ),
//! ```
//!
//! with `A` the admitted set and `F` the falsified set. Under the soundness
//! assumption (Assumption 1) falsifications are vacuous on the true universe,
//! so `Kₜ` is non-decreasing — the monotonicity Theorem 1(i) relies on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Identifier of a verifiable knowledge item (a certifiable atomic claim).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaimId(pub u64);

/// Identifier of a certifiable skill (an executable procedure with a testable
/// competence family).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillId(pub u64);

/// A finite importance measure over items.
///
/// The paper's default is the uniform counting measure (each item has mass
/// one); task-weighted measures are admissible. The trait is kept tiny so the
/// gap dynamics of Theorem 1 can be driven by either.
pub trait Measure<T> {
    /// Total mass of a set of items. Must be finite (Assumption 2) and
    /// additive over disjoint sets.
    fn mass<'a, I>(&self, items: I) -> f64
    where
        T: 'a,
        I: IntoIterator<Item = &'a T>;
}

/// The uniform counting measure: `mass(S) = |S|`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CountingMeasure;

impl<T> Measure<T> for CountingMeasure {
    fn mass<'a, I>(&self, items: I) -> f64
    where
        T: 'a,
        I: IntoIterator<Item = &'a T>,
    {
        // Count via the iterator; `f64` arithmetic so no integer overflow.
        let mut n = 0.0_f64;
        for _ in items {
            n += 1.0;
        }
        n
    }
}

/// The system state `x = (K, S)` of verified knowledge and certified skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct State {
    /// Verified knowledge set `K ⊆ K°`.
    pub knowledge: BTreeSet<ClaimId>,
    /// Certified skill set `S ⊆ S°`.
    pub skills: BTreeSet<SkillId>,
}

/// The admitted/falsified batch of one cycle: `A` and `F` of Eq. (2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Delta {
    /// Admitted knowledge `Aᴷ`.
    pub admit_knowledge: BTreeSet<ClaimId>,
    /// Admitted skills `Aˢ`.
    pub admit_skills: BTreeSet<SkillId>,
    /// Falsified knowledge `Fᴷ`.
    pub falsify_knowledge: BTreeSet<ClaimId>,
    /// Falsified skills `Fˢ`.
    pub falsify_skills: BTreeSet<SkillId>,
}

impl State {
    /// Construct an empty state `(∅, ∅)`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The metric `d(x, x′)` of Eq. (1) under the supplied measures.
    ///
    /// `d` is symmetric-difference mass; it is a genuine metric on `(X, d)`
    /// (symmetric, zero iff equal, triangle inequality) because the
    /// symmetric difference is, and the measures are finite.
    pub fn distance<MK, MS>(&self, other: &Self, mu: &MK, nu: &MS) -> f64
    where
        MK: Measure<ClaimId>,
        MS: Measure<SkillId>,
    {
        let k_sym: Vec<ClaimId> = self
            .knowledge
            .symmetric_difference(&other.knowledge)
            .copied()
            .collect();
        let s_sym: Vec<SkillId> = self
            .skills
            .symmetric_difference(&other.skills)
            .copied()
            .collect();
        mu.mass(k_sym.iter()) + nu.mass(s_sym.iter())
    }

    /// Apply one RSI cycle Φ (Eq. 2): union the admitted set, then remove the
    /// falsified set. Returns the next state `xₜ₊₁`.
    #[must_use]
    pub fn apply_phi(&self, delta: &Delta) -> Self {
        let mut knowledge = self.knowledge.clone();
        knowledge.extend(delta.admit_knowledge.iter().copied());
        for f in &delta.falsify_knowledge {
            knowledge.remove(f);
        }
        let mut skills = self.skills.clone();
        skills.extend(delta.admit_skills.iter().copied());
        for f in &delta.falsify_skills {
            skills.remove(f);
        }
        Self { knowledge, skills }
    }

    /// The epistemic gap `Gₜ = K° \ Kₜ` (Definition 4): items of the closure
    /// not yet in this state's knowledge. Returns the set; its mass `gₜ` is
    /// the contraction quantity of Theorem 1.
    #[must_use]
    pub fn knowledge_gap(&self, closure: &BTreeSet<ClaimId>) -> BTreeSet<ClaimId> {
        closure.difference(&self.knowledge).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(ids: &[u64]) -> BTreeSet<ClaimId> {
        ids.iter().map(|&i| ClaimId(i)).collect()
    }

    fn state(k: &[u64], s: &[u64]) -> State {
        State {
            knowledge: claims(k),
            skills: s.iter().map(|&i| SkillId(i)).collect(),
        }
    }

    #[test]
    fn distance_is_zero_iff_equal() {
        let a = state(&[1, 2, 3], &[1]);
        let b = a.clone();
        assert!((a.distance(&b, &CountingMeasure, &CountingMeasure)).abs() < 1e-12);
    }

    #[test]
    fn distance_counts_symmetric_difference() {
        let a = state(&[1, 2, 3], &[1]);
        let b = state(&[2, 3, 4], &[1, 2]);
        // knowledge △ = {1, 4} (mass 2); skills △ = {2} (mass 1) => 3.
        let d = a.distance(&b, &CountingMeasure, &CountingMeasure);
        assert!((d - 3.0).abs() < 1e-12, "d = {d}");
    }

    #[test]
    fn distance_is_symmetric() {
        let a = state(&[1, 2], &[]);
        let b = state(&[2, 3, 4], &[9]);
        let ab = a.distance(&b, &CountingMeasure, &CountingMeasure);
        let ba = b.distance(&a, &CountingMeasure, &CountingMeasure);
        assert!((ab - ba).abs() < 1e-12);
    }

    #[test]
    fn distance_obeys_triangle_inequality() {
        let a = state(&[1], &[]);
        let b = state(&[1, 2, 3], &[]);
        let c = state(&[3, 4, 5], &[1]);
        let ac = a.distance(&c, &CountingMeasure, &CountingMeasure);
        let ab = a.distance(&b, &CountingMeasure, &CountingMeasure);
        let bc = b.distance(&c, &CountingMeasure, &CountingMeasure);
        assert!(ac <= ab + bc + 1e-12);
    }

    #[test]
    fn phi_unions_admitted_then_removes_falsified() {
        let x = state(&[1, 2], &[]);
        let delta = Delta {
            admit_knowledge: claims(&[2, 3, 4]),
            falsify_knowledge: claims(&[1]),
            admit_skills: std::iter::once(SkillId(7)).collect(),
            falsify_skills: BTreeSet::new(),
        };
        let next = x.apply_phi(&delta);
        assert_eq!(next.knowledge, claims(&[2, 3, 4]));
        assert_eq!(next.skills, std::iter::once(SkillId(7)).collect());
    }

    #[test]
    fn phi_is_monotone_when_no_falsification() {
        // Assumption 1: falsifications vacuous on K° => K non-decreasing.
        let x = state(&[1, 2], &[]);
        let delta = Delta {
            admit_knowledge: claims(&[3, 4]),
            ..Delta::default()
        };
        let next = x.apply_phi(&delta);
        assert!(x.knowledge.is_subset(&next.knowledge));
    }

    #[test]
    fn gap_shrinks_as_state_grows() {
        let closure = claims(&[1, 2, 3, 4, 5]);
        let early = state(&[1, 2], &[]);
        let later = state(&[1, 2, 3, 4], &[]);
        assert_eq!(early.knowledge_gap(&closure).len(), 3);
        assert_eq!(later.knowledge_gap(&closure).len(), 1);
    }

    #[test]
    fn round_trips_through_serde() {
        let x = state(&[1, 2, 3], &[7, 8]);
        let json = serde_json::to_string(&x).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(x, back);
    }
}
