//! Hermes-shaped data types.
//!
//! The wire schema preserves the upstream Hermes
//! [`store.session`](https://github.com/NousResearch/hermes-agent) row
//! shape verbatim: `Session { id, created_ts, model, surface, title }`
//! and `Turn { id, session_id, parent_id, role, content, ts, taint,
//! cost }`. New material is appended in optional fields — every Hermes
//! consumer continues to parse a GaussClaw record without changes
//! (Binding Constraint #2 of `GAUSSCLAW_ROADMAP.md`).

use gauss_core::TaintLabel;
use serde::{Deserialize, Serialize};

/// Conversation session metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Session {
    /// Stable session id (UUID-shaped string).
    pub id: String,
    /// Creation timestamp (RFC3339).
    pub created: String,
    /// Surface that opened the session (`tui`, `rest`, `slack`, …).
    pub surface: String,
    /// Active model id at creation time.
    pub model: String,
    /// Optional human title (Hermes lets users name a session).
    #[serde(default)]
    pub title: String,
    /// Number of turns currently stored for this session.
    pub turn_count: u64,
}

impl Session {
    /// Build a fresh session with `now` timestamp.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        surface: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            created: now_rfc3339(),
            surface: surface.into(),
            model: model.into(),
            title: String::new(),
            turn_count: 0,
        }
    }
}

/// One conversation turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Turn {
    /// Monotonic turn id (per-store).
    pub id: u64,
    /// Owning session id.
    pub session_id: String,
    /// Parent turn id (`None` for session root).
    pub parent_id: Option<u64>,
    /// `"user"` | `"assistant"` | `"system"` | `"tool"`.
    pub role: String,
    /// Free-text body.
    pub content: String,
    /// RFC3339 timestamp.
    pub ts: String,
    /// Information-flow taint of this turn's input.
    pub taint: TaintLabel,
    /// Cost telemetry (Phase 4 populates the dollars/tokens fields).
    pub cost: TurnCost,
    /// **Meta-router transparency record.** Present iff the turn was
    /// dispatched through a [`gaussclaw_providers::RouterProvider`]
    /// (or equivalent meta-router): captures the candidate leaf set,
    /// the router's selection, and the actual model id reported by the
    /// chosen leaf. Absent for direct leaf dispatch.
    ///
    /// Because [`Turn`] is what the receipt chain hashes over, any
    /// after-the-fact edit to the route record diverges the chain head
    /// and fails [`crate::SessionStore::verify_chain`]. With an Ed25519
    /// signer attached, the signed receipt provides non-repudiation on
    /// the same bytes (Theorem T11). Hermes upstream has no equivalent
    /// transparency surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteRecord>,
}

/// Meta-router transparency record persisted with a routed turn.
///
/// Companion to [`Turn::route`]. Records what the router considered,
/// what it chose, and what actually answered — the three quantities
/// that together prove the router behaved transparently (paper
/// Theorem T7, "router-transparency").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RouteRecord {
    /// Leaf model ids the router considered, in catalogue order.
    pub candidates: Vec<String>,
    /// The leaf the router selected for this turn.
    pub selected: String,
    /// Model id reported by the chosen leaf in its [`Completion`].
    /// Should equal `selected` under a transparent router; a mismatch
    /// is a router-transparency violation and is recorded here so
    /// audit can detect it.
    ///
    /// [`Completion`]: gaussclaw_agent::Completion
    pub actual_model: String,
}

impl RouteRecord {
    /// Build a record from the three transparency-relevant strings.
    #[must_use]
    pub fn new(
        candidates: Vec<String>,
        selected: impl Into<String>,
        actual_model: impl Into<String>,
    ) -> Self {
        Self {
            candidates,
            selected: selected.into(),
            actual_model: actual_model.into(),
        }
    }

    /// True iff the router was transparent: the selected leaf id
    /// equals the model id the leaf reported back.
    #[must_use]
    pub fn is_transparent(&self) -> bool {
        self.selected == self.actual_model
    }
}

/// Cost telemetry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TurnCost {
    /// Prompt tokens accepted by the provider.
    pub prompt_tokens: u32,
    /// Completion tokens produced by the provider.
    pub completion_tokens: u32,
    /// Provider model that actually ran (may differ from the requested
    /// model under meta-routers).
    pub model_actual: String,
}

/// A search hit — the matched turn plus its relevance score.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct TurnHit {
    /// The matched turn.
    pub turn: Turn,
    /// Backend-specific relevance score.
    pub score: f32,
}

/// A lineage edge in the conversation graph.
///
/// Carries two integrity surfaces, chosen by deployment posture:
///
/// - **[`Self::commit_hex`]** — always present. BLAKE3 hex commit over
///   `(parent || child || chain-head-after-append)`. A hash commit:
///   any byte change in any of the three diverges it, AND because
///   `chain-head` is itself part of the receipt chain, this binds the
///   edge to the chain head.
///
/// - **[`Self::signature_hex`]** — present iff the store has an
///   attached Ed25519 signer. 128-char hex of the signature over
///   `(parent_le || child_le || chain-head)`. EUF-CMA non-repudiation
///   for the edge itself: an adversary without the secret key cannot
///   produce a verifying signature even if they recompute a valid
///   `commit_hex`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LineageEdge {
    /// Parent turn id.
    pub from: u64,
    /// Child turn id.
    pub to: u64,
    /// BLAKE3 hex commit (64 chars).
    pub commit_hex: String,
    /// Ed25519 signature hex (128 chars) when a signer is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_hex: Option<String>,
}

/// Snapshot of the receipt-chain head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChainHead {
    /// SHA-256 digest, hex-encoded (64 ASCII chars).
    pub digest_hex: String,
    /// Number of turns the digest covers.
    pub length: u64,
}

/// RFC3339 timestamp helper (UTC, second-precision).
#[must_use]
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
