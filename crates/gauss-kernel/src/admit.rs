//! Joint capability + taint admission — paper §VI.
//!
//! Phase 1 enforces `k ⪯ declass(ℓ) ⊓ Kt` with the two-bit refusal reason
//! mandated by SPECS §3.3.

use core::sync::atomic::{AtomicU64, Ordering};

use gauss_core::{GaussError, GaussResult, RefusalReason, TaintLabel};
use gauss_traits::Kernel;

use crate::cap::CapToken;
use crate::flow::{DeclassMap, DefaultDeclass, StrictDeclass};

/// Convenience for [`crate::admit::PrivilegedKernel::with_declass`] callers.
#[must_use]
pub const fn declass_default() -> DefaultDeclass {
    DefaultDeclass
}

/// Convenience for [`crate::admit::PrivilegedKernel::with_declass`] callers.
#[must_use]
pub const fn declass_strict() -> StrictDeclass {
    StrictDeclass
}

/// Privileged in-process kernel.
///
/// Phase 1 wires up:
///
/// * Current capability grant `K_t` stored in an `AtomicU64` so reads are
///   wait-free.
/// * A boxed [`DeclassMap`] (default: [`DefaultDeclass`]).
/// * The joint `admit` function and two-bit refusal reason.
pub struct PrivilegedKernel {
    grant: AtomicU64,
    declass: Box<dyn DeclassMap>,
}

impl core::fmt::Debug for PrivilegedKernel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PrivilegedKernel")
            .field(
                "grant",
                &CapToken::from_bits(self.grant.load(Ordering::Acquire)),
            )
            .field("declass", &"<dyn DeclassMap>")
            .finish()
    }
}

impl PrivilegedKernel {
    /// Construct a kernel with the [`DefaultDeclass`] map and the supplied
    /// initial capability grant.
    #[must_use]
    pub fn new(initial_grant: CapToken) -> Self {
        Self {
            grant: AtomicU64::new(initial_grant.bits()),
            declass: Box::new(DefaultDeclass),
        }
    }

    /// Construct a kernel with a custom declass map.
    pub fn with_declass<D: DeclassMap + 'static>(initial_grant: CapToken, declass: D) -> Self {
        Self {
            grant: AtomicU64::new(initial_grant.bits()),
            declass: Box::new(declass),
        }
    }

    /// Capability monotonicity (Axiom A2): contract the grant.
    ///
    /// The new grant MUST be `⪯` the current grant. Returns the new grant on
    /// success; returns an error if the supplied target is not below the
    /// current grant (which would be an implicit privilege escalation).
    ///
    /// # Errors
    /// Errors if `new_grant` is not `⪯` the current grant.
    pub fn contract(&self, new_grant: CapToken) -> GaussResult<CapToken> {
        loop {
            let current_bits = self.grant.load(Ordering::Acquire);
            let current = CapToken::from_bits(current_bits);
            if !new_grant.leq(current) {
                return Err(GaussError::Denied {
                    reason: RefusalReason::cap_only(),
                });
            }
            if self
                .grant
                .compare_exchange_weak(
                    current_bits,
                    new_grant.bits(),
                    Ordering::Release,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(new_grant);
            }
            // CAS contended — retry the whole sequence.
        }
    }
}

impl Kernel for PrivilegedKernel {
    fn current_grant(&self) -> CapToken {
        CapToken::from_bits(self.grant.load(Ordering::Acquire))
    }

    fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()> {
        let current = self.current_grant();
        let declass_bound = self.declass.declass(taint);
        let cap_ok = required.leq(current);
        let taint_ok = required.leq(declass_bound);
        if cap_ok && taint_ok {
            return Ok(());
        }
        let reason = RefusalReason {
            cap_bit: !cap_ok,
            taint_bit: !taint_ok,
        };
        Err(GaussError::Denied { reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_succeeds_for_a_trusted_call() {
        let k = PrivilegedKernel::new(CapToken::TOP);
        k.admit(CapToken::FILESYSTEM_READ, TaintLabel::Trusted)
            .unwrap();
    }

    #[test]
    fn admit_denies_post_under_web_taint() {
        let k = PrivilegedKernel::new(CapToken::TOP);
        let err = k
            .admit(CapToken::NETWORK_POST, TaintLabel::Web)
            .expect_err("POST must be denied under Web taint with the default declass");
        match err {
            GaussError::Denied { reason } => {
                assert!(reason.taint_bit, "taint bit must be set");
                assert!(!reason.cap_bit, "cap was sufficient; cap_bit must be unset");
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn admit_denies_post_when_cap_is_missing() {
        // Grant only NETWORK_GET; request POST under Trusted taint.
        let k = PrivilegedKernel::new(CapToken::NETWORK_GET);
        let err = k
            .admit(CapToken::NETWORK_POST, TaintLabel::Trusted)
            .expect_err("missing capability must deny");
        match err {
            GaussError::Denied { reason } => {
                assert!(reason.cap_bit);
                assert!(!reason.taint_bit);
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn contract_rejects_implicit_escalation() {
        let k = PrivilegedKernel::new(CapToken::NETWORK_GET);
        // Trying to expand to NETWORK_GET | NETWORK_POST must be denied.
        let target = CapToken::NETWORK_GET | CapToken::NETWORK_POST;
        let err = k.contract(target).expect_err("escalation must be denied");
        matches!(err, GaussError::Denied { .. });
    }

    #[test]
    fn contract_accepts_a_strict_drop() {
        let k = PrivilegedKernel::new(CapToken::NETWORK_GET | CapToken::FILESYSTEM_READ);
        let dropped = k.contract(CapToken::FILESYSTEM_READ).unwrap();
        assert_eq!(dropped, CapToken::FILESYSTEM_READ);
        assert_eq!(k.current_grant(), CapToken::FILESYSTEM_READ);
    }
}
