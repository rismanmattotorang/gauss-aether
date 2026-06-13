//! CriticAgent — uncertainty, curriculum, and the audit sampler (paper §IV.F).
//!
//! The critic quantifies uncertainty (self-consistency `p̂`), drives the
//! curriculum toward the *frontier band* where admission probability is
//! highest, and feeds the re-audit stream that bounds contamination
//! (Proposition 2). Drift detection (the Goal Drift Index) lives in
//! [`crate::gdi`]; this module supplies the remaining critic signals.
//!
//! The frontier filter concentrates cycles where `|p̂ − 0.5| ≤ δ`, exactly the
//! informative band Agent0 trains on [4] — these are the tasks whose
//! admission probability, hence the productivity `ρ` (Lemma 1, factor `β`), is
//! highest.

use serde::{Deserialize, Serialize};

/// Self-consistency `p̂` over a candidate set: the fraction of candidates that
/// agree with the modal (majority) answer (paper §IV.F, [62], [4]).
///
/// `answers` are opaque answer keys; the modal key's share is `p̂`. Returns
/// `0.0` for an empty set.
#[must_use]
pub fn self_consistency<T: Eq + std::hash::Hash>(answers: &[T]) -> f64 {
    if answers.is_empty() {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<&T, u32> = std::collections::HashMap::new();
    for a in answers {
        counts
            .entry(a)
            .and_modify(|n| *n = n.saturating_add(1))
            .or_insert(1);
    }
    let modal = counts.values().copied().max().unwrap_or(0);
    #[allow(clippy::cast_precision_loss)]
    let p = f64::from(modal) / answers.len() as f64;
    p
}

/// Whether a task is in the frontier band `|p̂ − 0.5| ≤ δ` (paper §IV.B/F).
///
/// Frontier tasks are neither trivial (`p̂ ≈ 1`) nor hopeless (`p̂ ≈ 0`), so
/// they carry the most information per cycle.
#[must_use]
pub fn in_frontier_band(p_hat: f64, delta: f64) -> bool {
    (p_hat - 0.5).abs() <= delta
}

/// Filter a batch of `(task_id, p̂)` pairs to the frontier band, best-first by
/// proximity to `p̂ = 0.5` (the curriculum selection of Algorithm 1 line 3).
#[must_use]
pub fn frontier_curriculum(tasks: &[(u64, f64)], delta: f64) -> Vec<u64> {
    let mut filtered: Vec<(u64, f64)> = tasks
        .iter()
        .copied()
        .filter(|&(_, p)| in_frontier_band(p, delta))
        .collect();
    filtered.sort_by(|a, b| {
        (a.1 - 0.5)
            .abs()
            .partial_cmp(&(b.1 - 0.5).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    filtered.into_iter().map(|(id, _)| id).collect()
}

/// A deterministic re-audit sampler (paper §IV.F, Proposition 2).
///
/// Selects stored items for re-verification at rate `η ∈ (0, 1]` per cycle.
/// The selection is a strided sample so it is reproducible by the conformance
/// suite rather than RNG-driven.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReauditSampler {
    /// Re-audit rate `η`.
    eta: f64,
    /// Running cursor into the item space (advances each cycle).
    cursor: u64,
}

impl ReauditSampler {
    /// Construct with re-audit rate `eta` (clamped into `[0, 1]`).
    #[must_use]
    pub fn new(eta: f64) -> Self {
        Self {
            eta: eta.clamp(0.0, 1.0),
            cursor: 0,
        }
    }

    /// Select roughly `η·|items|` item ids to re-audit this cycle, advancing
    /// the cursor so successive cycles cover the space (round-robin coverage).
    pub fn sample(&mut self, items: &[u64]) -> Vec<u64> {
        if items.is_empty() || self.eta <= 0.0 {
            return Vec::new();
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let take = ((items.len() as f64) * self.eta).ceil() as usize;
        let take = take.min(items.len());
        let start = usize::try_from(self.cursor)
            .unwrap_or(0)
            .checked_rem(items.len())
            .unwrap_or(0);
        let mut out = Vec::with_capacity(take);
        for offset in 0..take {
            let idx = start
                .checked_add(offset)
                .unwrap_or(0)
                .checked_rem(items.len())
                .unwrap_or(0);
            out.push(items[idx]);
        }
        self.cursor = self
            .cursor
            .saturating_add(u64::try_from(take).unwrap_or(u64::MAX));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_consistency_is_the_modal_share() {
        // 3 of 4 agree => p̂ = 0.75.
        let answers = ["a", "a", "a", "b"];
        assert!((self_consistency(&answers) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn self_consistency_of_unanimous_set_is_one() {
        let answers = ["x", "x", "x"];
        assert!((self_consistency(&answers) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn frontier_band_excludes_trivial_and_hopeless() {
        assert!(in_frontier_band(0.5, 0.1));
        assert!(in_frontier_band(0.45, 0.1));
        assert!(!in_frontier_band(0.95, 0.1)); // trivial
        assert!(!in_frontier_band(0.05, 0.1)); // hopeless
    }

    #[test]
    fn curriculum_keeps_frontier_tasks_closest_to_half_first() {
        let tasks = [(1, 0.95), (2, 0.52), (3, 0.5), (4, 0.1)];
        let picked = frontier_curriculum(&tasks, 0.1);
        // Tasks 2 and 3 are in-band; 3 (p̂=0.5) is closest to the frontier.
        assert_eq!(picked, vec![3, 2]);
    }

    #[test]
    fn reaudit_sampler_respects_the_rate() {
        let mut s = ReauditSampler::new(0.5);
        let items: Vec<u64> = (0..10).collect();
        let sample = s.sample(&items);
        assert_eq!(sample.len(), 5);
    }

    #[test]
    fn reaudit_sampler_covers_the_space_round_robin() {
        let mut s = ReauditSampler::new(0.5);
        let items: Vec<u64> = (0..4).collect();
        let first = s.sample(&items); // [0,1]
        let second = s.sample(&items); // [2,3]
        assert_ne!(first, second);
    }

    #[test]
    fn reaudit_disabled_at_zero_rate() {
        let mut s = ReauditSampler::new(0.0);
        assert!(s.sample(&[1, 2, 3]).is_empty());
    }
}
