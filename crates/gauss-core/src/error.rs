//! Unified error type for Gauss-Aether.
//!
//! Every public crate returns [`GaussError`] (or a `GaussResult`). The enum
//! is `#[non_exhaustive]` so new variants can be added without semver-major
//! churn.

use thiserror::Error;

/// Two-bit refusal reason for capability / taint denials.
///
/// Per SPECS §3.3 every refusal MUST tag *both* whether the capability bound
/// was insufficient and whether the taint bound was insufficient, so the
/// operator can distinguish authorisation failures from upstream-taint
/// failures.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct RefusalReason {
    /// `true` iff the capability check failed.
    pub cap_bit: bool,
    /// `true` iff the taint check failed.
    pub taint_bit: bool,
}

impl RefusalReason {
    /// Returns the refusal reason for a capability-only denial.
    #[must_use]
    pub const fn cap_only() -> Self {
        Self {
            cap_bit: true,
            taint_bit: false,
        }
    }

    /// Returns the refusal reason for a taint-only denial.
    #[must_use]
    pub const fn taint_only() -> Self {
        Self {
            cap_bit: false,
            taint_bit: true,
        }
    }

    /// Returns the refusal reason where both bounds failed simultaneously.
    #[must_use]
    pub const fn both() -> Self {
        Self {
            cap_bit: true,
            taint_bit: true,
        }
    }
}

/// Unified error type. Adding a variant is semver-minor (the enum is
/// `#[non_exhaustive]`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GaussError {
    /// Capability or taint check failed; see [`RefusalReason`].
    #[error("admissibility denied (cap_bit={}, taint_bit={})", reason.cap_bit, reason.taint_bit)]
    Denied {
        /// The two-bit refusal reason.
        reason: RefusalReason,
    },

    /// The supervised-autonomy gradient classified the action as `deny`.
    #[error("supervised autonomy classified action as deny")]
    AutonomyDenied,

    /// The supervised-autonomy approval queue timed out.
    #[error("supervised autonomy approval timed out")]
    AutonomyApprovalTimeout,

    /// Schema validation at the worker/parent boundary failed.
    #[error("schema validation failed: {0}")]
    SchemaValidation(String),

    /// The audit chain is broken (a link does not match `H(prev ‖ ρ)`).
    #[error("audit chain integrity check failed")]
    AuditChainBroken,

    /// Receipt signature verification failed.
    #[error("receipt signature verification failed")]
    ReceiptVerify,

    /// A worker recursion depth bound was exceeded (default 8).
    #[error("worker recursion depth exceeded (limit={limit})")]
    WorkerDepthExceeded {
        /// The depth limit that was exceeded.
        limit: u32,
    },

    /// I/O failure passed through verbatim.
    #[error("I/O failure: {0}")]
    Io(String),

    /// Catch-all for failures that have no kernel-defined variant yet.
    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_reason_combinations_are_distinct() {
        assert_ne!(RefusalReason::cap_only(), RefusalReason::taint_only());
        assert_ne!(RefusalReason::cap_only(), RefusalReason::both());
        assert_ne!(RefusalReason::taint_only(), RefusalReason::both());
    }

    #[test]
    fn denied_error_display_includes_both_bits() {
        let e = GaussError::Denied {
            reason: RefusalReason::both(),
        };
        let s = format!("{e}");
        assert!(s.contains("cap_bit=true"));
        assert!(s.contains("taint_bit=true"));
    }
}
