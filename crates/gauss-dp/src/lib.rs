//! `gauss-dp` — differentially-private trajectory exporter (v2 horizon,
//! paper §XVIII.E.3).
//!
//! Trajectories (sequences of receipt-chain entries) carry private user
//! information. The DP exporter perturbs aggregate statistics over a
//! batch of trajectories so the output is `(ε, δ)`-DP per Dwork et al.
//! 2014 — adding noise from one of two mechanisms:
//!
//! * **Laplace** — `f(x) + Lap(Δ / ε)` for `ε`-DP queries over
//!   bounded-L1-sensitivity functions.
//! * **Gaussian** — `f(x) + N(0, σ²)` with `σ ≥ Δ · sqrt(2 ln(1.25/δ)) / ε`
//!   for `(ε, δ)`-DP queries over bounded-L2-sensitivity functions.
//!
//! The crate exposes a [`Mechanism`] trait + two impls + a
//! [`PrivacyAccountant`] that tracks the cumulative `ε / δ` budget so a
//! single trajectory pipeline can't accidentally exceed its allocation.
//!
//! Production deployments wire a CSPRNG (e.g. `OsRng`); the conformance
//! suite uses a deterministic seeded RNG so the noise samples are
//! reproducible.

use core::f64::consts::TAU;

use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Differential-privacy mechanism trait.
pub trait Mechanism {
    /// Add noise to `value` such that the output is differentially
    /// private with the mechanism's `(ε, δ)` parameters and an L1 (for
    /// Laplace) or L2 (for Gaussian) sensitivity `delta_f`.
    ///
    /// `delta_f` is the maximum change in `value` between two adjacent
    /// trajectories (the standard DP "sensitivity").
    fn perturb<R: RngCore + CryptoRng>(&self, value: f64, delta_f: f64, rng: &mut R) -> f64;

    /// The `(ε, δ)` parameters this mechanism enforces.
    fn epsilon_delta(&self) -> (f64, f64);
}

/// Pure-Laplace mechanism, `ε`-DP (i.e. `δ = 0`).
#[derive(Debug, Clone, Copy)]
pub struct Laplace {
    /// Privacy budget.
    pub epsilon: f64,
}

impl Laplace {
    /// Build with `epsilon > 0`.
    #[must_use]
    pub const fn new(epsilon: f64) -> Self {
        Self { epsilon }
    }

    /// Sample one Laplace draw of scale `b`.
    fn sample<R: RngCore + CryptoRng>(b: f64, rng: &mut R) -> f64 {
        // Inverse-CDF sampling: draw u ∈ (0, 1), return
        // -b · sign(u - 0.5) · ln(1 - 2|u - 0.5|).
        let u = uniform_open(rng);
        let centered = u - 0.5;
        let sign = if centered.is_sign_positive() {
            1.0
        } else {
            -1.0
        };
        -b * sign * 2.0_f64.mul_add(-centered.abs(), 1.0).ln()
    }
}

impl Mechanism for Laplace {
    fn perturb<R: RngCore + CryptoRng>(&self, value: f64, delta_f: f64, rng: &mut R) -> f64 {
        if self.epsilon <= 0.0 || delta_f <= 0.0 {
            return value;
        }
        let scale = delta_f / self.epsilon;
        value + Self::sample(scale, rng)
    }

    fn epsilon_delta(&self) -> (f64, f64) {
        (self.epsilon, 0.0)
    }
}

/// Pure-Gaussian mechanism, `(ε, δ)`-DP.
#[derive(Debug, Clone, Copy)]
pub struct Gaussian {
    /// Privacy budget ε.
    pub epsilon: f64,
    /// Privacy slack δ.
    pub delta: f64,
}

impl Gaussian {
    /// Build with `epsilon > 0` and `delta in (0, 1)`.
    #[must_use]
    pub const fn new(epsilon: f64, delta: f64) -> Self {
        Self { epsilon, delta }
    }

    /// Box-Muller sample with mean `mu` and stddev `sigma`.
    fn sample<R: RngCore + CryptoRng>(mu: f64, sigma: f64, rng: &mut R) -> f64 {
        let u1 = uniform_open(rng);
        let u2 = uniform_open(rng);
        let mag = sigma * (-2.0 * u1.ln()).sqrt();
        mag.mul_add((TAU * u2).cos(), mu)
    }

    /// Standard analytic Gaussian bound: σ ≥ Δ · sqrt(2 ln(1.25/δ)) / ε.
    #[must_use]
    pub fn sigma_for(&self, delta_f: f64) -> f64 {
        if self.epsilon <= 0.0 || self.delta <= 0.0 || delta_f <= 0.0 {
            return 0.0;
        }
        delta_f * (2.0 * (1.25 / self.delta).ln()).sqrt() / self.epsilon
    }
}

impl Mechanism for Gaussian {
    fn perturb<R: RngCore + CryptoRng>(&self, value: f64, delta_f: f64, rng: &mut R) -> f64 {
        let sigma = self.sigma_for(delta_f);
        if sigma <= 0.0 {
            return value;
        }
        Self::sample(value, sigma, rng)
    }

    fn epsilon_delta(&self) -> (f64, f64) {
        (self.epsilon, self.delta)
    }
}

/// Privacy accountant tracking cumulative `(ε, δ)` spend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PrivacyAccountant {
    /// Total ε budget.
    pub epsilon_budget: f64,
    /// Total δ budget.
    pub delta_budget: f64,
    /// Cumulative ε spend.
    pub epsilon_spent: f64,
    /// Cumulative δ spend.
    pub delta_spent: f64,
}

impl PrivacyAccountant {
    /// Build with a fixed budget.
    #[must_use]
    pub const fn new(epsilon_budget: f64, delta_budget: f64) -> Self {
        Self {
            epsilon_budget,
            delta_budget,
            epsilon_spent: 0.0,
            delta_spent: 0.0,
        }
    }

    /// Charge a mechanism invocation. Composes via "basic composition":
    /// total budget = sum of per-query budgets.
    ///
    /// # Errors
    /// Returns [`DpError::BudgetExceeded`] when the spend would
    /// overshoot either budget.
    pub fn charge(&mut self, epsilon: f64, delta: f64) -> Result<(), DpError> {
        let new_eps = self.epsilon_spent + epsilon;
        let new_del = self.delta_spent + delta;
        if new_eps > self.epsilon_budget + f64::EPSILON
            || new_del > self.delta_budget + f64::EPSILON
        {
            return Err(DpError::BudgetExceeded {
                eps_after: new_eps,
                eps_budget: self.epsilon_budget,
                del_after: new_del,
                del_budget: self.delta_budget,
            });
        }
        self.epsilon_spent = new_eps;
        self.delta_spent = new_del;
        Ok(())
    }

    /// Remaining ε budget.
    #[must_use]
    pub fn epsilon_remaining(&self) -> f64 {
        (self.epsilon_budget - self.epsilon_spent).max(0.0)
    }
}

/// DP error.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum DpError {
    /// Charge would exceed the configured budget.
    #[error("privacy budget exceeded: ε {eps_after:.4}/{eps_budget:.4}, δ {del_after:.4}/{del_budget:.4}")]
    BudgetExceeded {
        /// Cumulative ε after the proposed charge.
        eps_after: f64,
        /// ε budget.
        eps_budget: f64,
        /// Cumulative δ after the proposed charge.
        del_after: f64,
        /// δ budget.
        del_budget: f64,
    },
}

/// Sample a uniform float in `(0, 1)` from a CSPRNG.
fn uniform_open<R: RngCore>(rng: &mut R) -> f64 {
    // Use 53 bits of randomness for a uniform [0, 1) float, then clamp
    // away the endpoints so callers can safely take `ln(u)` / `ln(1-u)`.
    let bits = rng.next_u64() >> 11; // 53 bits
    #[allow(clippy::cast_precision_loss)]
    let f = (bits as f64) / ((1_u64 << 53) as f64);
    f.clamp(f64::EPSILON, 1.0 - f64::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn det_rng() -> rand_core::OsRng {
        // OsRng is `CryptoRng`; the tests use it directly with seeded
        // helpers below for determinism where needed.
        rand_core::OsRng
    }

    /// Deterministic `ChaCha20`-style RNG so tests are reproducible.
    #[derive(Debug, Clone)]
    struct DetRng {
        state: u64,
    }
    impl DetRng {
        const fn new(seed: u64) -> Self {
            Self { state: seed }
        }
    }
    impl RngCore for DetRng {
        fn next_u32(&mut self) -> u32 {
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (self.state >> 32) as u32
        }
        fn next_u64(&mut self) -> u64 {
            (u64::from(self.next_u32()) << 32) | u64::from(self.next_u32())
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for b in dest.iter_mut() {
                *b = (self.next_u32() & 0xFF) as u8;
            }
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }
    impl CryptoRng for DetRng {}

    #[test]
    fn laplace_perturbs_within_a_few_scales() {
        let mech = Laplace::new(1.0);
        let mut rng = DetRng::new(42);
        let mut total = 0.0;
        let n = 1000;
        for _ in 0..n {
            total += (mech.perturb(0.0, 1.0, &mut rng)).abs();
        }
        let mean_abs = total / f64::from(n);
        // E[|Lap(0,1)|] = 1; mean over 1000 draws should be within 2x.
        assert!(mean_abs > 0.5 && mean_abs < 2.0, "mean_abs={mean_abs}");
    }

    #[test]
    fn gaussian_sigma_scales_with_sensitivity() {
        let mech = Gaussian::new(1.0, 1e-5);
        let s1 = mech.sigma_for(1.0);
        let s2 = mech.sigma_for(2.0);
        // σ is linear in Δ.
        assert!(2.0_f64.mul_add(-s1, s2).abs() < 1e-9);
    }

    #[test]
    fn accountant_charges_and_remains() {
        let mut acc = PrivacyAccountant::new(1.0, 1e-3);
        acc.charge(0.3, 0.0).unwrap();
        acc.charge(0.3, 0.0).unwrap();
        assert!((acc.epsilon_spent - 0.6).abs() < 1e-9);
        assert!((acc.epsilon_remaining() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn accountant_rejects_over_budget() {
        let mut acc = PrivacyAccountant::new(0.5, 0.0);
        acc.charge(0.3, 0.0).unwrap();
        let err = acc.charge(0.3, 0.0).unwrap_err();
        assert!(matches!(err, DpError::BudgetExceeded { .. }));
    }

    #[test]
    fn zero_epsilon_short_circuits_to_passthrough() {
        let mech = Laplace::new(0.0);
        let mut rng = DetRng::new(1);
        let out = mech.perturb(42.0, 1.0, &mut rng);
        assert!((out - 42.0).abs() < 1e-9);
    }

    #[test]
    fn osrng_works_at_compile_time() {
        // Smoke test that the trait works against a CryptoRng impl.
        let mech = Laplace::new(1.0);
        let mut rng = det_rng();
        let _ = mech.perturb(0.0, 1.0, &mut rng);
    }
}
