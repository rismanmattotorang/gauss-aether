//! Cost-aware LinUCB query router with an exploration floor (Algorithm 3).
//!
//! The QueryRouter implements `R` of Eq. (3): classify the task, select an
//! expert subset, and assign weights, trading verifier-measured utility
//! against price and latency per the cost-adjusted reward of Eq. (4):
//!
//! ```text
//! uᵢ(q) = Uᵢ(q) − λ$·cᵢ − λℓ·ℓᵢ ∈ [0, 1].
//! ```
//!
//! It is a disjoint-arms LinUCB bandit ([Li et al. 2010], [Chu et al. 2011])
//! with: (a) an `εₓ`-uniform exploration floor (needed by Lemma 1, charged to
//! regret in Theorem 3); (b) rewards computed *only after verification*, so
//! the bandit optimises certified — not fluent-sounding — utility; and (c)
//! soft fan-out to the UCB-near-optimal set with softmax weights, matching the
//! KKT structure of Eq. (14). The realised routing advantage `Δ̂R` of Eq. (13)
//! is recovered by [`routing_advantage`].
//!
//! Per-arm state `A⁻¹` is maintained directly via the Sherman–Morrison rank-1
//! update, so no matrix inversion runs in the hot path. The exploration draw
//! is supplied by the caller (a sampled value and arm index) rather than read
//! from a global RNG, keeping the router deterministic and replayable — the
//! same discipline the rest of the workspace applies to clocks.

use serde::{Deserialize, Serialize};

/// Cost-adjusted reward of Eq. (4): `u = U − λ$·c − λℓ·ℓ`, clamped to `[0, 1]`.
///
/// `utility` is the verifier-measured task utility `U ∈ [0, 1]`; `cost` and
/// `latency` are normalised price `c` and latency `ℓ`; `lambda_dollar` and
/// `lambda_latency` are the budget weights `λ$`, `λℓ`.
#[must_use]
pub fn cost_adjusted_reward(
    utility: f64,
    cost: f64,
    latency: f64,
    lambda_dollar: f64,
    lambda_latency: f64,
) -> f64 {
    let penalised = (-lambda_latency).mul_add(latency, lambda_dollar.mul_add(-cost, utility));
    penalised.clamp(0.0, 1.0)
}

/// One expert's share of a dispatch: the arm index and its fusion weight.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ArmWeight {
    /// Index of the selected expert in the router's arm list.
    pub arm: usize,
    /// Fusion weight `wᵢ` (the soft fan-out softmax weight; `1.0` for a pure
    /// single-arm dispatch).
    pub weight: f64,
}

/// The router's dispatch decision: the selected expert subset `I(q)` with
/// weights `{(Mᵢ, wᵢ)}`, plus whether this cycle was an exploration draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Dispatch {
    /// The weighted expert subset (weights sum to one).
    pub arms: Vec<ArmWeight>,
    /// Whether the dispatch was forced by the `εₓ` exploration floor.
    pub explored: bool,
}

/// A disjoint-arms cost-aware LinUCB router (Algorithm 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LinUcbRouter {
    /// Context dimension `d`.
    dim: usize,
    /// UCB exploration bonus `α`.
    alpha: f64,
    /// Exploration floor `εₓ` (probability of a uniform-random draw).
    epsilon_x: f64,
    /// Per-arm `A⁻¹` matrices (`d × d`, initialised to the identity).
    a_inv: Vec<Vec<Vec<f64>>>,
    /// Per-arm `b` vectors (`d`, initialised to zero).
    b: Vec<Vec<f64>>,
}

impl LinUcbRouter {
    /// Construct a router over `arms` experts with context dimension `dim`,
    /// UCB bonus `alpha`, and exploration floor `epsilon_x`.
    ///
    /// # Panics
    /// Panics if `arms == 0` or `dim == 0` — a router needs at least one arm
    /// and a non-trivial context.
    #[must_use]
    pub fn new(arms: usize, dim: usize, alpha: f64, epsilon_x: f64) -> Self {
        assert!(arms > 0, "router needs at least one arm");
        assert!(dim > 0, "router needs a non-trivial context dimension");
        let identity = identity(dim);
        Self {
            dim,
            alpha,
            epsilon_x: epsilon_x.clamp(0.0, 1.0),
            a_inv: vec![identity; arms],
            b: vec![vec![0.0; dim]; arms],
        }
    }

    /// Number of experts (arms).
    #[must_use]
    pub fn arms(&self) -> usize {
        self.b.len()
    }

    /// Ridge-regression estimate `θ̂ᵢ = A⁻¹ᵢ bᵢ` for arm `i`.
    #[must_use]
    fn theta(&self, arm: usize) -> Vec<f64> {
        mat_vec(&self.a_inv[arm], &self.b[arm])
    }

    /// The UCB score `θ̂ᵢᵀφ + α·√(φᵀA⁻¹ᵢφ)` for arm `i` on context `phi`.
    #[must_use]
    pub fn ucb(&self, arm: usize, phi: &[f64]) -> f64 {
        let theta = self.theta(arm);
        let mean = dot(&theta, phi);
        let a_inv_phi = mat_vec(&self.a_inv[arm], phi);
        let variance = dot(phi, &a_inv_phi).max(0.0);
        self.alpha.mul_add(variance.sqrt(), mean)
    }

    /// Greedy route (Algorithm 3, lines 4–8, no exploration): pick the
    /// UCB-maximal arm, admit every arm within `gamma` of the maximum,
    /// truncate to fan-out `m`, and assign softmax weights over the UCB scores.
    #[must_use]
    pub fn route_greedy(&self, phi: &[f64], fanout: usize, gamma: f64) -> Dispatch {
        let ucbs: Vec<f64> = (0..self.arms()).map(|i| self.ucb(i, phi)).collect();
        let best = ucbs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        // Near-optimal set I(q) = { i : ucbᵢ ≥ ucb_best − γ }, best-first.
        let mut near: Vec<usize> = (0..self.arms())
            .filter(|&i| ucbs[i] >= best - gamma)
            .collect();
        near.sort_by(|&a, &b| {
            ucbs[b]
                .partial_cmp(&ucbs[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        near.truncate(fanout.max(1));
        // Softmax weights wᵢ ∝ exp(ucbᵢ) over the selected set.
        let max_sel = near
            .iter()
            .map(|&i| ucbs[i])
            .fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = near.iter().map(|&i| (ucbs[i] - max_sel).exp()).collect();
        let z: f64 = exps.iter().sum();
        let arms = near
            .iter()
            .zip(exps.iter())
            .map(|(&arm, &e)| ArmWeight {
                arm,
                weight: if z > 0.0 { e / z } else { 0.0 },
            })
            .collect();
        Dispatch {
            arms,
            explored: false,
        }
    }

    /// Full route (Algorithm 3, lines 2–8) with the exploration floor.
    ///
    /// `explore_draw ∈ [0, 1)` and `explore_arm` are caller-supplied: when
    /// `explore_draw < εₓ` the router dispatches the single uniform-random
    /// `explore_arm` with weight one (the coverage draw Lemma 1 needs);
    /// otherwise it defers to [`Self::route_greedy`].
    #[must_use]
    pub fn route(
        &self,
        phi: &[f64],
        fanout: usize,
        gamma: f64,
        explore_draw: f64,
        explore_arm: usize,
    ) -> Dispatch {
        if explore_draw < self.epsilon_x {
            let arm = explore_arm.checked_rem(self.arms()).unwrap_or(0);
            return Dispatch {
                arms: vec![ArmWeight { arm, weight: 1.0 }],
                explored: true,
            };
        }
        self.route_greedy(phi, fanout, gamma)
    }

    /// Update arm `arm` with observed context `phi` and post-verification
    /// reward `r` (Algorithm 3, line 9): `bᵢ += r·φ` and the Sherman–Morrison
    /// rank-1 refresh of `A⁻¹ᵢ`.
    pub fn update(&mut self, arm: usize, phi: &[f64], reward: f64) {
        if arm >= self.arms() || phi.len() != self.dim {
            return;
        }
        // b += r·φ.
        for (bi, &p) in self.b[arm].iter_mut().zip(phi.iter()) {
            *bi += reward * p;
        }
        // Sherman–Morrison: A⁻¹ -= (A⁻¹φ)(A⁻¹φ)ᵀ / (1 + φᵀA⁻¹φ).
        let u = mat_vec(&self.a_inv[arm], phi);
        let denom = 1.0 + dot(phi, &u);
        if denom.abs() < f64::EPSILON {
            return;
        }
        for (i, row) in self.a_inv[arm].iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell -= (u[i] * u[j]) / denom;
            }
        }
    }
}

/// Empirical routing advantage `Δ̂R` of Eq. (13): the realised routed utility
/// minus the best single-arm counterfactual, averaged over queries.
///
/// `routed` is the realised cost-adjusted utility of the routed choice on each
/// query; `per_arm` is the counterfactual utility of every arm on each query
/// (`per_arm[q][i]`). Returns `Δ̂R = mean_q(routed) − maxᵢ mean_q(uᵢ)`, which
/// is non-negative when routing beats the best fixed expert (strictly so under
/// expert heterogeneity, Theorem 3(ii)). Returns `0.0` on empty input.
#[must_use]
pub fn routing_advantage(routed: &[f64], per_arm: &[Vec<f64>]) -> f64 {
    if routed.is_empty() || per_arm.is_empty() {
        return 0.0;
    }
    let n = routed.len();
    #[allow(clippy::cast_precision_loss)]
    let n_f = n as f64;
    let routed_mean = routed.iter().sum::<f64>() / n_f;
    let arms = per_arm.iter().map(Vec::len).max().unwrap_or(0);
    let mut best_arm_mean = f64::NEG_INFINITY;
    for i in 0..arms {
        let mut acc = 0.0;
        for q in per_arm {
            acc += q.get(i).copied().unwrap_or(0.0);
        }
        best_arm_mean = best_arm_mean.max(acc / n_f);
    }
    routed_mean - best_arm_mean
}

/// `d × d` identity matrix.
fn identity(dim: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; dim]; dim];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// Matrix–vector product `M·v`.
fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter().map(|row| dot(row, v)).collect()
}

/// Inner product `⟨a, b⟩` over the shared prefix length.
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_adjusted_reward_penalises_price_and_latency() {
        let full = cost_adjusted_reward(1.0, 0.0, 0.0, 0.15, 0.05);
        let penalised = cost_adjusted_reward(1.0, 1.0, 1.0, 0.15, 0.05);
        assert!((full - 1.0).abs() < 1e-12);
        assert!((penalised - 0.8).abs() < 1e-12);
    }

    #[test]
    fn reward_is_clamped_into_unit_interval() {
        let r = cost_adjusted_reward(0.1, 1.0, 1.0, 0.9, 0.9);
        assert!((0.0..=1.0).contains(&r));
        assert!(r.abs() < 1e-12);
    }

    #[test]
    fn greedy_route_weights_sum_to_one() {
        let r = LinUcbRouter::new(4, 3, 0.6, 0.05);
        let d = r.route_greedy(&[1.0, 0.0, 0.0], 3, 0.03);
        let sum: f64 = d.arms.iter().map(|a| a.weight).sum();
        assert!((sum - 1.0).abs() < 1e-9, "weights sum {sum}");
        assert!(!d.explored);
    }

    #[test]
    fn fanout_caps_the_dispatch_size() {
        let r = LinUcbRouter::new(5, 3, 0.6, 0.05);
        let d = r.route_greedy(&[0.5, 0.5, 0.5], 2, 100.0);
        assert!(d.arms.len() <= 2);
    }

    #[test]
    fn exploration_floor_forces_a_single_uniform_arm() {
        let r = LinUcbRouter::new(4, 3, 0.6, 0.1);
        // draw below εₓ => explore the supplied arm.
        let d = r.route(&[1.0, 0.0, 0.0], 3, 0.03, 0.01, 2);
        assert!(d.explored);
        assert_eq!(d.arms.len(), 1);
        assert_eq!(d.arms[0].arm, 2);
        // draw above εₓ => greedy.
        let g = r.route(&[1.0, 0.0, 0.0], 3, 0.03, 0.9, 2);
        assert!(!g.explored);
    }

    #[test]
    fn learning_raises_the_reward_estimate_for_a_rewarded_arm() {
        let mut r = LinUcbRouter::new(2, 2, 0.0, 0.0); // α=0 => pure exploitation
        let phi = [1.0, 0.0];
        let before = r.ucb(0, &phi);
        // Reward arm 0 repeatedly on this context.
        for _ in 0..5 {
            r.update(0, &phi, 1.0);
        }
        let after = r.ucb(0, &phi);
        assert!(after > before, "estimate did not rise: {before} -> {after}");
    }

    #[test]
    fn learned_arm_beats_unrewarded_arm() {
        let mut r = LinUcbRouter::new(2, 2, 0.0, 0.0);
        let phi = [1.0, 0.0];
        for _ in 0..10 {
            r.update(0, &phi, 1.0);
            r.update(1, &phi, 0.0);
        }
        let d = r.route_greedy(&phi, 1, 0.0);
        assert_eq!(d.arms[0].arm, 0);
    }

    #[test]
    fn routing_advantage_is_nonnegative_for_per_query_best() {
        // Router that always realises the per-query max utility.
        let per_arm = vec![vec![0.2, 0.9], vec![0.8, 0.1], vec![0.5, 0.6]];
        let routed: Vec<f64> = per_arm
            .iter()
            .map(|q| q.iter().copied().fold(f64::NEG_INFINITY, f64::max))
            .collect();
        let dr = routing_advantage(&routed, &per_arm);
        assert!(dr >= -1e-12, "ΔR = {dr}");
        assert!(
            dr > 0.0,
            "heterogeneous pool should give strictly positive ΔR"
        );
    }

    #[test]
    fn routing_advantage_is_zero_when_one_arm_dominates() {
        // Arm 0 is best on every query => routing can't beat the best fixed arm.
        let per_arm = vec![vec![0.9, 0.1], vec![0.8, 0.2], vec![0.7, 0.3]];
        let routed = vec![0.9, 0.8, 0.7];
        let dr = routing_advantage(&routed, &per_arm);
        assert!(dr.abs() < 1e-12, "ΔR = {dr}");
    }

    #[test]
    fn round_trips_through_serde() {
        let r = LinUcbRouter::new(3, 4, 0.6, 0.05);
        let json = serde_json::to_string(&r).unwrap();
        let back: LinUcbRouter = serde_json::from_str(&json).unwrap();
        assert_eq!(back.arms(), 3);
    }
}
