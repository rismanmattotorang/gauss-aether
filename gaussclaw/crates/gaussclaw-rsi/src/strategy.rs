//! `LinUcbStrategy` — the Gauss-Agent0 router as a live `SelectionStrategy`.
//!
//! Plugs the cost-aware LinUCB router (paper Algorithm 3, Theorem 3) into the
//! `gaussclaw-providers-meta` `NotDiamondProvider` so the live agent routes
//! across the frozen model pool by verifier-measured utility. The strategy
//! featurizes the prompt deterministically, scores each candidate model by its
//! UCB, and returns the argmax — with [`LinUcbStrategy::reward`] feeding the
//! post-verification reward (Eq. 4) back into the bandit.

use std::sync::Mutex;

use gauss_rsi::router::{cost_adjusted_reward, LinUcbRouter};
use gaussclaw_agent::Prompt;
use gaussclaw_providers_meta::SelectionStrategy;

/// Context feature dimension for prompt featurization.
const FEATURE_DIM: usize = 16;

/// A learned [`SelectionStrategy`] backed by a [`LinUcbRouter`].
pub struct LinUcbStrategy {
    router: Mutex<LinUcbRouter>,
    /// Arm index → model slug (the catalogue order the router was built over).
    arms: Vec<String>,
    /// Budget weights for the cost-adjusted reward (Eq. 4).
    lambda_dollar: f64,
    lambda_latency: f64,
}

impl LinUcbStrategy {
    /// Build over an ordered slug catalogue. The router is sized to one arm
    /// per slug, with a fixed context dimension.
    #[must_use]
    pub fn new(arms: Vec<String>, alpha: f64, epsilon_x: f64) -> Self {
        let n = arms.len().max(1);
        Self {
            router: Mutex::new(LinUcbRouter::new(n, FEATURE_DIM, alpha, epsilon_x)),
            arms,
            lambda_dollar: 0.15,
            lambda_latency: 0.05,
        }
    }

    /// Deterministic prompt featurization: a byte-bucket histogram over the
    /// model id and the last user message, L1-normalized.
    fn featurize(prompt: &Prompt) -> Vec<f64> {
        let mut v = vec![0.0_f64; FEATURE_DIM];
        let mut text = prompt.model.clone();
        if let Some(last) = prompt.messages.iter().rev().find(|m| m.role == "user") {
            text.push(' ');
            text.push_str(&last.content);
        }
        for byte in text.as_bytes() {
            let idx = (*byte as usize) % FEATURE_DIM;
            if let Some(slot) = v.get_mut(idx) {
                *slot += 1.0;
            }
        }
        let total: f64 = v.iter().sum();
        if total > 0.0 {
            for slot in &mut v {
                *slot /= total;
            }
        }
        v
    }

    /// Arm index of a model slug, if known to this strategy.
    fn arm_of(&self, slug: &str) -> Option<usize> {
        self.arms.iter().position(|s| s == slug)
    }

    /// Feed a post-verification reward for `slug` back into the bandit
    /// (Eq. 4 with this strategy's budget weights).
    pub fn reward(&self, slug: &str, prompt: &Prompt, utility: f64, cost: f64, latency: f64) {
        let Some(arm) = self.arm_of(slug) else {
            return;
        };
        let r = cost_adjusted_reward(
            utility,
            cost,
            latency,
            self.lambda_dollar,
            self.lambda_latency,
        );
        let feat = Self::featurize(prompt);
        if let Ok(mut router) = self.router.lock() {
            router.update(arm, &feat, r);
        }
    }
}

impl SelectionStrategy for LinUcbStrategy {
    fn select(&self, prompt: &Prompt, candidates: &[String]) -> Option<String> {
        let feat = Self::featurize(prompt);
        // Score every known candidate by UCB, releasing the router lock before
        // the final selection (keeps the guard's lifetime minimal).
        let scored: Vec<(f64, String)> = {
            let router = self.router.lock().ok()?;
            candidates
                .iter()
                .filter_map(|cand| {
                    self.arm_of(cand)
                        .map(|arm| (router.ucb(arm, &feat), cand.clone()))
                })
                .collect()
        };
        let best = scored
            .into_iter()
            .fold(None, |acc: Option<(f64, String)>, item| match acc {
                Some((score, _)) if score >= item.0 => acc,
                _ => Some(item),
            });
        best.map(|(_, slug)| slug)
            .or_else(|| candidates.first().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_agent::Message;

    fn prompt(model: &str) -> Prompt {
        Prompt::new(model, vec![Message::new("user", "compute a sum")])
    }

    #[test]
    fn selects_a_known_candidate() {
        let strat = LinUcbStrategy::new(
            vec!["openai/gpt-4o".into(), "anthropic/claude".into()],
            0.6,
            0.0,
        );
        let chosen = strat.select(
            &prompt("router"),
            &["openai/gpt-4o".into(), "anthropic/claude".into()],
        );
        assert!(chosen.is_some());
        assert!(strat.arm_of(&chosen.unwrap()).is_some());
    }

    #[test]
    fn rewarded_arm_is_preferred() {
        let arms: Vec<String> = vec!["a/x".into(), "b/y".into()];
        let strat = LinUcbStrategy::new(arms.clone(), 0.0, 0.0); // pure exploitation
        let p = prompt("router");
        // Reward arm "a/x" repeatedly; it should win selection.
        for _ in 0..10 {
            strat.reward("a/x", &p, 1.0, 0.0, 0.0);
            strat.reward("b/y", &p, 0.0, 0.0, 0.0);
        }
        let chosen = strat.select(&p, &arms);
        assert_eq!(chosen.as_deref(), Some("a/x"));
    }

    #[test]
    fn unknown_candidate_falls_back_to_first() {
        let strat = LinUcbStrategy::new(vec!["known/model".into()], 0.6, 0.0);
        let chosen = strat.select(&prompt("router"), &["unknown/model".into()]);
        assert_eq!(chosen.as_deref(), Some("unknown/model"));
    }
}
