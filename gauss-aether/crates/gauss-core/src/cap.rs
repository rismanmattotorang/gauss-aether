//! Capability lattice `K` — paper Axiom 2, Theorem T2.
//!
//! `CapToken` lives in `gauss-core` so that `Action`, the kernel, the memory
//! backend, and plugin crates can all reference it without a circular
//! dependency on `gauss-kernel`. `gauss-kernel` re-exports it.
//!
//! The lattice is a finite bitmask over a fixed namespace (SPECS §4.1).
//! Phase 1 onward locks the namespace; new bits are an ADR-gated change.

use core::ops::{BitAnd, BitOr};

use serde::{Deserialize, Serialize};

/// A finite capability token, encoded as a bitmask over a fixed namespace.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapToken(u64);

impl CapToken {
    /// The bottom element `⊥` — no capabilities.
    pub const BOTTOM: Self = Self(0);
    /// The top element `⊤` — every capability in the namespace.
    pub const TOP: Self = Self(u64::MAX);

    // Canonical capability bits. The exact assignments are not stable across
    // versions but the constants themselves are.

    /// Read from a scoped filesystem path.
    pub const FILESYSTEM_READ: Self = Self(1 << 0);
    /// Write to a scoped filesystem path.
    pub const FILESYSTEM_WRITE: Self = Self(1 << 1);
    /// Perform an HTTP GET to an allow-listed origin.
    pub const NETWORK_GET: Self = Self(1 << 2);
    /// Perform an HTTP POST to an allow-listed origin.
    pub const NETWORK_POST: Self = Self(1 << 3);
    /// Spawn a child process (sandboxed).
    pub const SUBPROCESS_SPAWN: Self = Self(1 << 4);
    /// Use a long-lived crypto key for signing.
    pub const CRYPTO_SIGN: Self = Self(1 << 5);
    /// Render to the A2UI Live Canvas.
    pub const CANVAS_RENDER: Self = Self(1 << 6);
    /// Embed an iframe from an allow-listed origin.
    pub const CANVAS_EMBED: Self = Self(1 << 7);
    /// Push a file to the user's downloads.
    pub const CANVAS_FILE_WRITE: Self = Self(1 << 8);
    /// Read an environment variable from a caller-supplied allowlist.
    /// The cap admits *membership* in the allowlist, not access to any
    /// particular variable — the tool implementation enforces the
    /// per-variable check.
    pub const ENV_READ: Self = Self(1 << 9);
    /// Read past conversations from the session store (FTS / HNSW
    /// hybrid recall). Refused under `Adversarial` taint by the
    /// default declass map, so a web-fetched message cannot query the
    /// user's history.
    pub const MEMORY_READ: Self = Self(1 << 10);
    /// Open an approval / clarification prompt on the operator's
    /// behalf. Cap-gated so a low-privilege sub-agent can't surface a
    /// modal pretending to be the parent agent.
    pub const APPROVAL_ASK: Self = Self(1 << 11);

    /// Construct from a raw bitmask.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Return the raw bitmask.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Lattice meet (`⊓`).
    #[inline]
    #[must_use]
    pub const fn meet(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }

    /// Lattice join (`⊔`).
    #[inline]
    #[must_use]
    pub const fn join(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }

    /// True iff `self ⪯ rhs`.
    #[inline]
    #[must_use]
    pub const fn leq(self, rhs: Self) -> bool {
        (self.0 & rhs.0) == self.0
    }

    /// True iff this token contains every bit in `rhs`.
    #[inline]
    #[must_use]
    pub const fn contains(self, rhs: Self) -> bool {
        (self.0 & rhs.0) == rhs.0
    }
}

impl BitAnd for CapToken {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        self.meet(rhs)
    }
}

impl BitOr for CapToken {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        self.join(rhs)
    }
}

/// Trait alias for things that behave like a capability. Phase 1 promotes
/// this to the kernel's public sealed-trait surface.
pub trait Capability: Copy + Eq {
    /// Lattice meet.
    #[must_use]
    fn meet(self, rhs: Self) -> Self;
    /// Lattice join.
    #[must_use]
    fn join(self, rhs: Self) -> Self;
    /// Order: `self ⪯ rhs`.
    fn leq(self, rhs: Self) -> bool;
}

impl Capability for CapToken {
    #[inline]
    fn meet(self, rhs: Self) -> Self {
        Self::meet(self, rhs)
    }
    #[inline]
    fn join(self, rhs: Self) -> Self {
        Self::join(self, rhs)
    }
    #[inline]
    fn leq(self, rhs: Self) -> bool {
        Self::leq(self, rhs)
    }
}
