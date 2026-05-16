//! `gauss-traits` — public trait surface.
//!
//! Plugin authors depend on this crate (and `gauss-core`) only; the kernel
//! depends on this crate to consume implementations. The eight traits enumerated
//! by SPECS §11 land here as they are designed.
//!
//! Phase 1 stabilises three: [`Kernel`], [`MemoryBackend`], and [`Sealed`].
//! Future phases add `ProviderTrait`, `ChannelTrait`, `ToolTrait`,
//! `SandboxTrait`, `VoiceTrait`, `ApprovalTrait`, `CanvasTrait`.

use async_trait::async_trait;
use gauss_core::{Action, CapToken, GaussResult, Observation, TaintLabel, TurnId};
use serde::{Deserialize, Serialize};

/// Sealed marker — prevents downstream crates from implementing kernel-private
/// traits. Place an unimplemented marker in every sealed trait to force the
/// pattern: `trait Foo: Sealed { ... }`.
pub mod sealed {
    /// The seal. Implement only inside the workspace.
    pub trait Sealed {}
}

pub use sealed::Sealed;

/// Kernel surface (Phase 1).
///
/// Implementations are not user-pluggable — there is one privileged kernel per
/// process. The trait references [`CapToken`] directly (which lives in
/// `gauss-core`), so this crate has no dependency edge on `gauss-kernel`.
pub trait Kernel: Send + Sync {
    /// Look up the agent's current capability grant.
    fn current_grant(&self) -> CapToken;

    /// Joint capability/taint admission. Returns the refusal reason on denial
    /// (encoded as the appropriate [`gauss_core::GaussError`] variant).
    ///
    /// The function is total: every action that would otherwise execute MUST
    /// pass through `admit` first.
    ///
    /// # Errors
    /// Returns [`gauss_core::GaussError::Denied`] when either the capability
    /// or taint bound is not satisfied.
    fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()>;
}

/// Memory monoid surface (Phase 1).
///
/// Real implementations live in `gauss-memory` (in-memory + `SurrealDB`). The
/// trait is `async_trait` because `SurrealDB` calls are async.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Append a record. The implementation MUST ensure durability before
    /// returning (Axiom A1).
    async fn append(&self, entry: AppendEntry) -> GaussResult<AppendAck>;

    /// Current chain head (Phase 5 onward — Phase 1 returns the genesis or a
    /// stub hash).
    async fn chain_head(&self) -> GaussResult<ChainHeadSnapshot>;

    /// Number of records currently in the log.
    async fn len(&self) -> GaussResult<u64>;

    /// True iff `len() == 0`. Default implementation queries `len`.
    async fn is_empty(&self) -> GaussResult<bool> {
        Ok(self.len().await? == 0)
    }
}

/// Audit record being appended.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AppendEntry {
    /// Identifier of the originating turn.
    pub turn_id: TurnId,
    /// Opaque record payload — Phase 2 will replace with a typed delta.
    pub payload: Vec<u8>,
    /// Taint of the record's underlying observation(s).
    pub taint: TaintLabel,
}

impl AppendEntry {
    /// Construct an append entry.
    #[must_use]
    pub const fn new(turn_id: TurnId, payload: Vec<u8>, taint: TaintLabel) -> Self {
        Self {
            turn_id,
            payload,
            taint,
        }
    }
}

/// Acknowledgement of a successful append.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AppendAck {
    /// Position of the appended record in the log (0-based).
    pub index: u64,
    /// Chain head after the append.
    pub head: ChainHeadSnapshot,
}

impl AppendAck {
    /// Construct an acknowledgement. Provided because the struct is
    /// `#[non_exhaustive]` and cannot be struct-literal'd from outside this
    /// crate.
    #[must_use]
    pub const fn new(index: u64, head: ChainHeadSnapshot) -> Self {
        Self { index, head }
    }
}

/// Snapshot of the chain head returned by [`MemoryBackend::chain_head`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChainHeadSnapshot {
    /// SHA-256 digest of the chain at the time of the snapshot.
    pub digest: [u8; 32],
    /// Number of records the digest covers.
    pub length: u64,
}

impl ChainHeadSnapshot {
    /// Genesis snapshot (zero digest, length 0).
    pub const GENESIS: Self = Self {
        digest: [0u8; 32],
        length: 0,
    };

    /// Construct a snapshot. Provided because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(digest: [u8; 32], length: u64) -> Self {
        Self { digest, length }
    }
}

/// LLM policy trait `π` (Phase 2).
///
/// A `Provider` consumes an observation history (Phase 2 sees only the most
/// recent observation; Phase 6 will add full prefix retrieval) and emits a
/// sequence of actions. Real providers (Anthropic, `OpenAI`, Google) ship in
/// Phase 8 once the polyhedral-equivalence verifier is in place.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Generate one turn's worth of actions for the given observation.
    ///
    /// # Errors
    /// Implementations return [`GaussError::Io`] on transport failures and
    /// [`GaussError::Internal`] on policy / parser failures.
    ///
    /// [`GaussError::Io`]: gauss_core::GaussError::Io
    /// [`GaussError::Internal`]: gauss_core::GaussError::Internal
    async fn generate(&self, obs: &Observation) -> GaussResult<Vec<Action>>;
}
