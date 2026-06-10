//! Anchor cadence policy (paper §IX.D).
//!
//! Anchoring every chain append into an external timestamp authority is
//! expensive (HTTP RTT) and unnecessary for tamper-evidence — one anchor
//! every `N` appends is enough to bound the rewind window to `N` records.
//!
//! [`AnchorPolicy`] expresses the cadence; [`Anchorer`] drives a
//! [`TsaClient`] against a running chain.

use std::sync::Arc;

use gauss_core::GaussResult;
use tokio::sync::Mutex;

use crate::chain::ChainHead;
use crate::tsa::{Anchor, TsaClient};

/// How often the chain head should be anchored.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct AnchorPolicy {
    /// Anchor every `every_n_appends` records. `1` anchors every append
    /// (high cost, low rewind window); `1000` is the SPECS §IX.D default.
    pub every_n_appends: u64,
}

impl Default for AnchorPolicy {
    fn default() -> Self {
        Self::SPECS_DEFAULT
    }
}

impl AnchorPolicy {
    /// SPECS §IX.D default: anchor every 1000 appends.
    pub const SPECS_DEFAULT: Self = Self {
        every_n_appends: 1000,
    };

    /// Anchor every append. High-cost, smallest rewind window.
    pub const EVERY_APPEND: Self = Self { every_n_appends: 1 };

    /// Build a policy with a custom cadence (must be `>= 1`).
    #[must_use]
    pub const fn every(n: u64) -> Self {
        Self {
            every_n_appends: if n == 0 { 1 } else { n },
        }
    }

    /// True iff an anchor should be emitted at the given 1-based append count.
    #[must_use]
    pub const fn should_anchor_at(&self, count: u64) -> bool {
        count != 0 && count.is_multiple_of(self.every_n_appends)
    }
}

/// Drives anchoring against a [`TsaClient`] under an [`AnchorPolicy`].
///
/// `Anchorer` owns its policy + client and tracks the most recent anchor (so
/// the operator can query "what was the last externally-witnessed head?").
pub struct Anchorer<C: TsaClient> {
    client: C,
    policy: AnchorPolicy,
    last_anchor: Arc<Mutex<Option<Anchor>>>,
}

impl<C: TsaClient> core::fmt::Debug for Anchorer<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Anchorer")
            .field("policy", &self.policy)
            .field("client_kind", &self.client.kind())
            .finish_non_exhaustive()
    }
}

impl<C: TsaClient> Anchorer<C> {
    /// Wrap `client` with the given cadence.
    pub fn new(client: C, policy: AnchorPolicy) -> Self {
        Self {
            client,
            policy,
            last_anchor: Arc::new(Mutex::new(None)),
        }
    }

    /// Borrow the underlying TSA client.
    pub const fn client(&self) -> &C {
        &self.client
    }

    /// Active policy.
    pub const fn policy(&self) -> AnchorPolicy {
        self.policy
    }

    /// Most recent anchor produced. `None` until the cadence first fires.
    pub async fn last_anchor(&self) -> Option<Anchor> {
        self.last_anchor.lock().await.clone()
    }

    /// Maybe anchor the chain head. Called by the engine after each append;
    /// returns `Some(anchor)` when the cadence fires and `None` otherwise.
    ///
    /// `chain_count` is the 1-based number of appends so far (i.e. the new
    /// chain length after the append the caller is committing).
    ///
    /// # Errors
    /// Propagates [`TsaClient::anchor`] failures verbatim.
    pub async fn maybe_anchor(
        &self,
        head: ChainHead,
        chain_count: u64,
    ) -> GaussResult<Option<Anchor>> {
        if !self.policy.should_anchor_at(chain_count) {
            return Ok(None);
        }
        // `chain_count` is the new length; the index of the appended record
        // is `chain_count - 1` (0-based).
        let index = chain_count.saturating_sub(1);
        let anchor = self.client.anchor(head, index).await?;
        *self.last_anchor.lock().await = Some(anchor.clone());
        Ok(Some(anchor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tsa::SimulatorTsaClient;

    #[test]
    fn default_policy_is_specs_default() {
        assert_eq!(AnchorPolicy::default(), AnchorPolicy::SPECS_DEFAULT);
        assert_eq!(AnchorPolicy::SPECS_DEFAULT.every_n_appends, 1000);
    }

    #[test]
    fn should_anchor_fires_at_multiples_of_n() {
        let p = AnchorPolicy::every(5);
        assert!(!p.should_anchor_at(0));
        assert!(!p.should_anchor_at(1));
        assert!(!p.should_anchor_at(4));
        assert!(p.should_anchor_at(5));
        assert!(!p.should_anchor_at(6));
        assert!(p.should_anchor_at(10));
        assert!(p.should_anchor_at(1_000_000));
    }

    #[test]
    fn every_treats_zero_as_one() {
        assert_eq!(AnchorPolicy::every(0).every_n_appends, 1);
        assert_eq!(AnchorPolicy::every(1).every_n_appends, 1);
    }

    #[tokio::test]
    async fn anchorer_only_fires_on_cadence() {
        let client = SimulatorTsaClient::from_seed([1u8; 32]).with_fixed_clock(42);
        let anchorer = Anchorer::new(client, AnchorPolicy::every(3));
        let head = ChainHead::from_bytes([0u8; 32]);
        assert!(anchorer.maybe_anchor(head, 1).await.unwrap().is_none());
        assert!(anchorer.maybe_anchor(head, 2).await.unwrap().is_none());
        let anchor = anchorer.maybe_anchor(head, 3).await.unwrap().unwrap();
        assert_eq!(anchor.anchored_at_index, 2);
        assert!(anchorer.maybe_anchor(head, 4).await.unwrap().is_none());
        let anchor2 = anchorer.maybe_anchor(head, 6).await.unwrap().unwrap();
        assert_eq!(anchor2.anchored_at_index, 5);
        // The anchorer tracks the most recent anchor.
        assert_eq!(anchorer.last_anchor().await.unwrap(), anchor2);
    }
}
