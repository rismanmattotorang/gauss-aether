//! [`SessionStore`] — the Hermes-shaped query surface.
//!
//! Wraps `gauss_memory::SurrealMemory` (chain-protected append log,
//! BM25 FTS index, HNSW vector index, SHA-256 Merkle chain head) and
//! adds in-memory indices for the Hermes-shaped queries (sessions,
//! lineage parent↔child, recent-sessions list).
//!
//! ## Atomicity model
//!
//! Every `append_turn` call:
//!
//! 1. Locks the in-memory state mutex.
//! 2. Allocates a monotonic `turn_id`.
//! 3. Serialises the Hermes-shaped [`Turn`] into the append-log payload.
//! 4. Calls [`gauss_memory::SurrealMemory::append`], which advances the
//!    SHA-256 chain head atomically.
//! 5. Mirrors the turn into the in-memory indices (sessions, parent
//!    map, FTS/HNSW results cache).
//! 6. Releases the lock.
//!
//! The append log is the canonical record. The in-memory indices are
//! rebuildable from the log on restart (the rebuild path is exercised
//! by [`SessionStore::verify_chain`]).
//!
//! ## Tamper-evidence
//!
//! Two paths:
//!
//! - **In-memory tamper** — modify a [`Turn`] in the indices, then call
//!   `verify_chain()`. The reconstructed head diverges from the live
//!   `SurrealMemory` head.
//! - **Persistent tamper** — change a serialised payload in the
//!   underlying database. The live head computed from the persistent
//!   log diverges from any externally-anchored head (TSA proof, Phase
//!   2 slice 4).
//!
//! Hermes upstream has no equivalent — its SQLite-FTS5 store has no
//! Merkle structure, so neither tamper class is detectable.

use std::collections::HashMap;
use std::sync::Arc;

use gauss_audit::{Anchor, Ed25519Signer, ReceiptSigner, SignedReceipt, TsaClient};
use gauss_core::TurnId;
use gauss_memory::SurrealMemory;
use gauss_traits::{AppendEntry, HybridQuery, MemoryBackend, RecallHit};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::embed::mock_embed;
use crate::types::{
    now_rfc3339, ChainHead, LineageEdge, RouteRecord, Session, Turn, TurnCost, TurnHit,
};

// ─── errors ────────────────────────────────────────────────────────────────

/// Store-side error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The underlying memory backend refused.
    #[error("backend: {0:?}")]
    Backend(#[from] gauss_core::GaussError),
    /// Requested session is not in the store.
    #[error("unknown session: {0}")]
    UnknownSession(String),
    /// Requested turn is not in the store.
    #[error("unknown turn: {0}")]
    UnknownTurn(u64),
    /// Serialisation failed.
    #[error("serialise: {0}")]
    Serde(#[from] serde_json::Error),
    /// Chain verification: the reconstructed digest differs from the live one.
    #[error("chain divergence at index {at}: local digest {local} != backend digest {backend}")]
    ChainDivergence {
        /// Length at which divergence was detected.
        at: u64,
        /// Hex of the locally-reconstructed digest.
        local: String,
        /// Hex of the backend's live digest.
        backend: String,
    },
}

/// Convenience result alias.
pub type StoreResult<T> = Result<T, StoreError>;

// ─── store ─────────────────────────────────────────────────────────────────

/// Hermes-shaped session / turn / lineage store atop the chain-
/// protected SurrealDB Trinity backend.
pub struct SessionStore {
    memory: Arc<SurrealMemory>,
    pub(crate) state: Mutex<State>,
    /// Optional Ed25519 receipt signer. When attached, every
    /// [`Self::append_turn`] also produces a [`SignedReceipt`] proving
    /// non-repudiation under EUF-CMA (Theorem T11 of the source paper).
    signer: Option<Arc<ReceiptSigner<Ed25519Signer>>>,
    /// Optional TSA client for periodic wall-clock anchoring of the
    /// chain head. Operators call [`Self::anchor_now`] manually or
    /// run a background loop (slice 7+ ships the loop).
    tsa: Option<Arc<dyn TsaClient>>,
}

#[derive(Default)]
pub(crate) struct State {
    /// session_id → metadata
    pub(crate) sessions: HashMap<String, Session>,
    /// turn_id → full Turn record
    pub(crate) turns: HashMap<u64, Turn>,
    /// session_id → ordered list of turn ids
    pub(crate) session_turns: HashMap<String, Vec<u64>>,
    /// parent_turn_id → child turn ids (empty entry when no children yet)
    pub(crate) parent_children: HashMap<u64, Vec<u64>>,
    /// Lineage edges keyed by child turn id.
    pub(crate) lineage: HashMap<u64, LineageEdge>,
    /// Ed25519-signed receipts keyed by turn id. Only populated when a
    /// signer is attached to the store.
    pub(crate) receipts: HashMap<u64, SignedReceipt>,
    /// Persisted payload bytes for receipt verification (the exact bytes
    /// the signer signed over).
    pub(crate) receipt_payloads: HashMap<u64, Vec<u8>>,
    /// TSA anchors of the chain head over time (insert-ordered).
    pub(crate) anchors: Vec<Anchor>,
    /// Next turn id to allocate.
    pub(crate) next_turn_id: u64,
}

impl SessionStore {
    /// Build a fresh store over the embedded in-memory SurrealDB.
    /// Useful for tests, the CLI demo, and the Phase 1 web/desktop
    /// dashboards. Production deployments call [`Self::with_memory`]
    /// with a persistent backend.
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] if the SurrealDB instance fails
    /// to start.
    pub async fn open_in_memory() -> StoreResult<Self> {
        let memory = SurrealMemory::open_in_memory().await?;
        Ok(Self::with_memory(Arc::new(memory)))
    }

    /// Build a store over a **persistent** embedded SurrealKV backend
    /// rooted at `path`. Sessions, the lineage graph, and the receipt
    /// chain survive process restarts; reopening the same path restores
    /// the chain head so new turns extend the existing chain.
    ///
    /// Requires the `kv-surrealkv` feature.
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] if the on-disk store can't be
    /// opened or its persisted state can't be restored.
    #[cfg(feature = "kv-surrealkv")]
    pub async fn open_surrealkv(path: impl AsRef<std::path::Path>) -> StoreResult<Self> {
        let memory = SurrealMemory::open_surrealkv(path).await?;
        Ok(Self::with_memory(Arc::new(memory)))
    }

    /// Build a store over a caller-supplied [`SurrealMemory`] handle.
    #[must_use]
    pub fn with_memory(memory: Arc<SurrealMemory>) -> Self {
        Self {
            memory,
            state: Mutex::new(State::default()),
            signer: None,
            tsa: None,
        }
    }

    /// Attach a [`TsaClient`] for wall-clock anchoring. Operators
    /// trigger anchors via [`Self::anchor_now`]; a periodic background
    /// loop is left to the deployment (see `gaussclaw-bin` for the
    /// reference wiring).
    #[must_use]
    pub fn with_tsa(mut self, tsa: Arc<dyn TsaClient>) -> Self {
        self.tsa = Some(tsa);
        self
    }

    /// Anchor the current chain head with the attached TSA, append the
    /// returned anchor to the in-memory anchor history, and return it.
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] when no TSA is attached or the
    /// TSA call fails.
    pub async fn anchor_now(&self) -> StoreResult<Anchor> {
        let tsa = self.tsa.as_ref().ok_or_else(|| {
            StoreError::Backend(gauss_core::GaussError::AnchorFailed(
                "no TSA client attached".into(),
            ))
        })?;
        let snap = self.memory.chain_head().await?;
        let head = gauss_audit::ChainHead::from_bytes(snap.digest);
        let anchor = tsa.anchor(head, snap.length).await?;
        self.state.lock().await.anchors.push(anchor.clone());
        Ok(anchor)
    }

    /// All anchors held in memory, in insertion order.
    pub async fn anchors(&self) -> Vec<Anchor> {
        self.state.lock().await.anchors.clone()
    }

    /// Attach an Ed25519 receipt signer. Every subsequent
    /// [`Self::append_turn`] produces a signed, verifiable receipt.
    /// Until a signer is attached, the store operates in the chain-
    /// protected-but-unsigned mode (Hermes-equivalent on integrity,
    /// stronger on tamper-evidence; signing adds non-repudiation).
    #[must_use]
    pub fn with_signer(mut self, signer: Arc<ReceiptSigner<Ed25519Signer>>) -> Self {
        self.signer = Some(signer);
        self
    }

    /// The signer's public verifying key, if a signer is attached.
    #[must_use]
    pub fn public_key(&self) -> Option<[u8; 32]> {
        self.signer.as_ref().map(|s| {
            // Disambiguate from the inherent `Ed25519Signer::public_key`
            // (which returns `&[u8; 32]`) — we want the SigningBackend
            // trait method which returns by value.
            <gauss_audit::Ed25519Signer as gauss_audit::SigningBackend>::public_key(s.backend())
        })
    }

    /// Borrow the underlying memory backend.
    #[must_use]
    pub fn memory(&self) -> &SurrealMemory {
        &self.memory
    }

    // ─── sessions ───────────────────────────────────────────────────────────

    /// Create a new session and persist its metadata in the in-memory index.
    pub async fn create_session(
        &self,
        surface: impl Into<String>,
        model: impl Into<String>,
    ) -> Session {
        let id = next_session_id();
        let sess = Session::new(id.clone(), surface, model);
        let mut st = self.state.lock().await;
        st.sessions.insert(id.clone(), sess.clone());
        st.session_turns.insert(id, Vec::new());
        sess
    }

    /// Look up a session.
    pub async fn get_session(&self, id: &str) -> Option<Session> {
        self.state.lock().await.sessions.get(id).cloned()
    }

    /// List sessions newest-first.
    pub async fn list_recent_sessions(&self, limit: usize) -> Vec<Session> {
        let st = self.state.lock().await;
        let mut all: Vec<Session> = st.sessions.values().cloned().collect();
        // Sort newest-first by `created` (lexicographic RFC3339 = chronological).
        all.sort_by(|a, b| b.created.cmp(&a.created));
        all.truncate(limit);
        all
    }

    // ─── turns ──────────────────────────────────────────────────────────────

    /// Append a turn to a session.
    ///
    /// Atomically: serialises the [`Turn`], appends it to the
    /// chain-protected log (advancing the SHA-256 Merkle head),
    /// mirrors it into the in-memory indices, and signs a
    /// [`LineageEdge`] if `parent_id` is present.
    ///
    /// # Errors
    /// Returns [`StoreError::UnknownSession`] if the session was not
    /// created beforehand; [`StoreError::Backend`] on backend failure;
    /// [`StoreError::Serde`] on serialise failure (should not happen
    /// with a well-formed [`Turn`]).
    pub async fn append_turn(
        &self,
        session_id: impl Into<String>,
        parent_id: Option<u64>,
        role: impl Into<String>,
        content: impl Into<String>,
        taint: gauss_core::TaintLabel,
    ) -> StoreResult<(Turn, ChainHead)> {
        self.append_turn_inner(
            session_id.into(),
            parent_id,
            role.into(),
            content.into(),
            taint,
            None,
        )
        .await
    }

    /// Append a turn that was dispatched through a meta-router.
    ///
    /// Same atomicity contract as [`Self::append_turn`], with one
    /// addition: the [`RouteRecord`] is persisted in the turn payload,
    /// so it falls under the same chain-protected, optionally Ed25519-
    /// signed integrity surface as the turn content itself. Verifies
    /// the paper's Theorem-T7 router-transparency property post hoc:
    /// any consumer can compare `record.selected` to
    /// `record.actual_model`, and the bytes can't be edited after the
    /// fact without diverging the chain head.
    ///
    /// # Errors
    /// Same as [`Self::append_turn`].
    pub async fn append_routed_turn(
        &self,
        session_id: impl Into<String>,
        parent_id: Option<u64>,
        role: impl Into<String>,
        content: impl Into<String>,
        taint: gauss_core::TaintLabel,
        route: RouteRecord,
    ) -> StoreResult<(Turn, ChainHead)> {
        self.append_turn_inner(
            session_id.into(),
            parent_id,
            role.into(),
            content.into(),
            taint,
            Some(route),
        )
        .await
    }

    async fn append_turn_inner(
        &self,
        session_id: String,
        parent_id: Option<u64>,
        role: String,
        content: String,
        taint: gauss_core::TaintLabel,
        route: Option<RouteRecord>,
    ) -> StoreResult<(Turn, ChainHead)> {
        // Hold the state mutex across the await so the chain head and
        // the in-memory mirror stay consistent: two concurrent appends
        // must serialise.
        let mut st = self.state.lock().await;
        if !st.sessions.contains_key(&session_id) {
            return Err(StoreError::UnknownSession(session_id));
        }

        let turn_id = st.next_turn_id.saturating_add(1);
        st.next_turn_id = turn_id;

        // If routed, mirror the chosen leaf into TurnCost.model_actual so
        // existing Hermes-shape consumers (which only know about
        // TurnCost) still see the routed model id without parsing the
        // route record.
        let cost = route.as_ref().map_or_else(TurnCost::default, |r| TurnCost {
            prompt_tokens: 0,
            completion_tokens: 0,
            model_actual: r.actual_model.clone(),
        });

        let turn = Turn {
            id: turn_id,
            session_id: session_id.clone(),
            parent_id,
            role,
            content: content.clone(),
            ts: now_rfc3339(),
            taint,
            cost,
            route,
        };

        let payload = serde_json::to_vec(&turn)?;
        // Capture the prev_head BEFORE the append so we can sign the
        // exact chain transition the receipt witnesses.
        let prev_head_snap = self.memory.chain_head().await?;
        let prev_head_arr: [u8; 32] = prev_head_snap.digest;
        let prev_head = gauss_audit::ChainHead::from_bytes(prev_head_arr);
        let entry = AppendEntry::new(TurnId::new(u128::from(turn_id)), payload.clone(), taint)
            .with_text(content)
            .with_embedding(mock_embed(&turn.content));
        let ack = self.memory.append(entry).await?;

        // Optional Ed25519 signature over (turn_id, index, prev_head,
        // payload_digest, post_head, taint, signed_at_ms). A consumer
        // can verify the receipt with the stored payload and the
        // public key — no trust in the store needed.
        if let Some(signer) = &self.signer {
            let signed_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let receipt = signer.sign_append(
                TurnId::new(u128::from(turn_id)),
                ack.index,
                prev_head,
                &payload,
                taint,
                signed_at_ms,
            )?;
            st.receipts.insert(turn_id, receipt);
            st.receipt_payloads.insert(turn_id, payload);
        }

        // Commit + (optionally) sign the lineage edge.
        //
        // The BLAKE3 hash commit binds parent, child, and the chain head
        // — any byte change in any of those three diverges it. When an
        // Ed25519 signer is attached, the same canonical bytes are also
        // Ed25519-signed; the signature provides EUF-CMA non-repudiation
        // on top of the chain-head binding.
        if let Some(p) = parent_id {
            let mut canonical = Vec::with_capacity(8 + 8 + 32);
            canonical.extend_from_slice(&p.to_le_bytes());
            canonical.extend_from_slice(&turn_id.to_le_bytes());
            canonical.extend_from_slice(&ack.head.digest);
            let commit_hex = blake3::hash(&canonical).to_hex().to_string();
            let signature_hex = if let Some(signer) = &self.signer {
                use gauss_audit::SigningBackend;
                let sig = signer.backend().sign(&canonical)?;
                Some(hex_string(&sig))
            } else {
                None
            };
            let edge = LineageEdge {
                from: p,
                to: turn_id,
                commit_hex,
                signature_hex,
            };
            st.lineage.insert(turn_id, edge);
            st.parent_children.entry(p).or_default().push(turn_id);
        }

        st.turns.insert(turn_id, turn.clone());
        st.session_turns
            .entry(session_id.clone())
            .or_default()
            .push(turn_id);
        if let Some(sess) = st.sessions.get_mut(&session_id) {
            sess.turn_count = sess.turn_count.saturating_add(1);
        }
        drop(st);

        let head = ChainHead {
            digest_hex: hex_string(&ack.head.digest),
            length: ack.head.length,
        };
        Ok((turn, head))
    }

    /// Get one turn by id.
    pub async fn get_turn(&self, id: u64) -> Option<Turn> {
        self.state.lock().await.turns.get(&id).cloned()
    }

    /// List a session's turns in append order.
    pub async fn list_session_turns(&self, session_id: &str) -> Vec<Turn> {
        let st = self.state.lock().await;
        let Some(ids) = st.session_turns.get(session_id) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| st.turns.get(id).cloned())
            .collect()
    }

    // ─── lineage ────────────────────────────────────────────────────────────

    /// Walk from `turn_id` toward the session root, returning the
    /// chain in order `[turn_id, parent, grandparent, ..., root]`.
    pub async fn lineage_to_root(&self, turn_id: u64) -> Vec<Turn> {
        let st = self.state.lock().await;
        let mut out = Vec::new();
        let mut cursor = Some(turn_id);
        while let Some(id) = cursor {
            let Some(t) = st.turns.get(&id).cloned() else {
                break;
            };
            cursor = t.parent_id;
            out.push(t);
        }
        out
    }

    /// Return the immediate children of `turn_id`.
    pub async fn lineage_children(&self, turn_id: u64) -> Vec<Turn> {
        let st = self.state.lock().await;
        st.parent_children
            .get(&turn_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| st.turns.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Look up the signed [`LineageEdge`] for a child turn.
    pub async fn lineage_edge(&self, child_id: u64) -> Option<LineageEdge> {
        self.state.lock().await.lineage.get(&child_id).cloned()
    }

    // ─── signed receipts ────────────────────────────────────────────────────

    /// Look up the signed receipt for a turn. Returns `None` when no
    /// signer is attached or the turn is unknown.
    pub async fn get_receipt(&self, turn_id: u64) -> Option<SignedReceipt> {
        self.state.lock().await.receipts.get(&turn_id).cloned()
    }

    /// Verify the signed receipt for a turn against the stored payload.
    /// Returns `Ok(false)` if there is no receipt (e.g. no signer
    /// attached) or no payload recorded; `Ok(true)` on successful
    /// verification; `Err` on signature / digest mismatch.
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] wrapping the underlying
    /// `gauss-audit::verify` error when the signature does not bind
    /// the stored payload.
    pub async fn verify_receipt(&self, turn_id: u64) -> StoreResult<bool> {
        let (receipt, payload) = {
            let st = self.state.lock().await;
            let Some(r) = st.receipts.get(&turn_id).cloned() else {
                return Ok(false);
            };
            let Some(p) = st.receipt_payloads.get(&turn_id).cloned() else {
                return Ok(false);
            };
            (r, p)
        };
        receipt.verify(&payload)?;
        Ok(true)
    }

    // ─── search ─────────────────────────────────────────────────────────────

    /// BM25 keyword search over turn content.
    pub async fn fts_search(&self, query: &str, limit: usize) -> StoreResult<Vec<TurnHit>> {
        let hits = self.memory.fts_search(query, limit).await?;
        Ok(self.materialize_hits(hits).await)
    }

    /// HNSW vector search for the deterministic mock embedding of `query`.
    /// Phase 4 swaps the mock embedding for a provider model.
    pub async fn vector_search(&self, query_text: &str, k: usize) -> StoreResult<Vec<TurnHit>> {
        let q = mock_embed(query_text);
        let hits = self.memory.vector_search(&q, k).await?;
        Ok(self.materialize_hits(hits).await)
    }

    /// Hybrid BM25 ∪ HNSW recall (Theorem T5 of GaussClaw.pdf — union
    /// recall miss-rate `ε_fts · ε_vec`).
    pub async fn hybrid_search(
        &self,
        query_text: &str,
        k: usize,
        alpha: f32,
    ) -> StoreResult<Vec<TurnHit>> {
        let q = HybridQuery::new(
            Some(query_text.to_string()),
            Some(mock_embed(query_text)),
            k,
            alpha,
        );
        let hits = self.memory.hybrid_recall(q).await?;
        Ok(self.materialize_hits(hits).await)
    }

    async fn materialize_hits(&self, hits: Vec<RecallHit>) -> Vec<TurnHit> {
        let st = self.state.lock().await;
        hits.into_iter()
            .filter_map(|h| {
                // Turn ids fit into u64 (Hermes-compat); the wider TurnId
                // u128 is downcast at the lookup boundary.
                let key = u64::try_from(h.turn_id.as_u128()).ok()?;
                st.turns.get(&key).cloned().map(|turn| TurnHit {
                    turn,
                    score: h.score,
                })
            })
            .collect()
    }

    // ─── chain ──────────────────────────────────────────────────────────────

    /// Current chain head as held by the backend.
    pub async fn chain_head(&self) -> StoreResult<ChainHead> {
        let snap = self.memory.chain_head().await?;
        Ok(ChainHead {
            digest_hex: hex_string(&snap.digest),
            length: snap.length,
        })
    }

    /// Reconstruct the chain locally from the in-memory mirror and
    /// compare against the backend's live head. Diverges iff the
    /// mirror has been tampered with OR the persistent log has been
    /// tampered with — both are detectable.
    ///
    /// # Errors
    /// Returns [`StoreError::ChainDivergence`] on mismatch.
    pub async fn verify_chain(&self) -> StoreResult<()> {
        let st = self.state.lock().await;
        let mut head = gauss_audit::ChainHead::ZERO;
        // Replay turns in insertion order — this is the canonical ordering
        // used by `SurrealMemory::append`.
        let mut ids: Vec<u64> = st.turns.keys().copied().collect();
        ids.sort_unstable();
        let mut length: u64 = 0;
        for id in ids {
            let turn = st.turns.get(&id).expect("just enumerated");
            let payload = serde_json::to_vec(turn)?;
            head = gauss_audit::link(head, &payload);
            length = length.saturating_add(1);
        }
        drop(st);

        let live = self.memory.chain_head().await?;
        if head.as_bytes() != &live.digest || length != live.length {
            return Err(StoreError::ChainDivergence {
                at: length,
                local: hex_string(head.as_bytes()),
                backend: hex_string(&live.digest),
            });
        }
        Ok(())
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn hex_string(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().saturating_mul(2));
    for byte in b {
        s.push(nibble(byte >> 4));
        s.push(nibble(byte & 0x0F));
    }
    s
}

#[allow(clippy::arithmetic_side_effects)]
const fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => '0',
    }
}

fn next_session_id() -> String {
    // Cheap unique id: 16 random-ish hex chars derived from the current
    // nanosecond + a thread-local atomic counter. Sufficient for the
    // in-process store; production deployments use UUIDv7.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&count.to_le_bytes());
    hasher
        .finalize()
        .to_hex()
        .to_string()
        .chars()
        .take(16)
        .collect()
}
