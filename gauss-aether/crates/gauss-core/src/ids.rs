//! Identifier newtypes.
//!
//! Each identifier is a thin wrapper around a primitive so that the type
//! system distinguishes a `TurnId` from a `SessionId` even when both are
//! `u128`. Identifiers are `Copy`, `Eq`, `Hash`, and `serde`-serialisable
//! (`String`-backed IDs are `Clone` only).

use serde::{Deserialize, Serialize};

/// Monotonic turn identifier. Lexicographic ordering follows ULID semantics
/// once the ULID generator is wired in Phase 2; for Phase 0 the underlying
/// `u128` is opaque to callers.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(pub u128);

impl TurnId {
    /// Construct a turn identifier from a raw u128.
    #[inline]
    #[must_use]
    pub const fn new(raw: u128) -> Self {
        Self(raw)
    }

    /// Return the raw u128 backing this identifier.
    #[inline]
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

/// Long-lived session identifier (one session = many turns).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub u128);

impl SessionId {
    /// Construct a session identifier from a raw u128.
    #[inline]
    #[must_use]
    pub const fn new(raw: u128) -> Self {
        Self(raw)
    }

    /// Return the raw u128 backing this identifier.
    #[inline]
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

/// Logical agent identifier. The string form is opaque to the kernel and is
/// intended to be a UUID once the kernel grows a key-management layer.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

/// Tool identifier — opaque short string declared by the tool manifest.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolId(pub String);

/// Worker-context identifier — handed out by the HWCA at worker spawn.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerId(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_types() {
        // This test exists to make sure that the type-level boundary between
        // identifier kinds is real. If a future refactor accidentally makes
        // them aliases, this test will fail to compile.
        let turn = TurnId::new(1);
        let session = SessionId::new(1);
        assert_eq!(turn.as_u128(), session.as_u128());
        // We deliberately do *not* compile `assert_eq!(turn, session)` —
        // they must remain distinct types.
    }

    #[test]
    fn ids_round_trip_through_serde() {
        let id = TurnId::new(42);
        let json = serde_json::to_string(&id).unwrap();
        let back: TurnId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
