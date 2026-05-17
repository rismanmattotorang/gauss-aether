//! `gaussclaw-store` — Hermes-shaped session and lineage store atop
//! the SurrealDB Trinity substrate.
//!
//! Phase 2 of `GAUSSCLAW_ROADMAP.md`. Replaces upstream Hermes's
//! `store.session` (SQLite + FTS5) and `store.lineage` (parent-pointer
//! recursive-CTE traversal) with a structurally tamper-evident
//! design.
//!
//! ## Six superiorities over the Hermes upstream
//!
//! 1. **Chain-protected appends (Theorem T3).** Every turn write
//!    advances a SHA-256 Merkle chain head. Any byte changed in any
//!    past turn diverges the head — verifiable in O(n) via
//!    [`SessionStore::verify_chain`]. Hermes SQLite rows are mutable
//!    with no integrity surface.
//!
//! 2. **BM25 ∪ HNSW hybrid recall (Theorem T5).** The store exposes
//!    FTS, vector, and hybrid search. Hermes only has FTS5. Union
//!    recall miss-rate `ε_fts · ε_vec` — strictly better than either
//!    channel alone (proved in the source paper).
//!
//! 3. **Signed lineage edges.** Every parent→child edge carries a
//!    BLAKE3 hex of `(parent || child || chain-head-at-append)` —
//!    detects tampering in three places. Hermes's parent-pointer
//!    table has no signature.
//!
//! 4. **Atomicity model is explicit.** The mutex held over the
//!    append serialises chain advancement and the in-memory index
//!    update. No half-state on failure. Hermes uses SQLite
//!    transactions but the lineage table is a separate write that
//!    can interleave.
//!
//! 5. **Async-native.** `tokio::sync::Mutex` + async backend
//!    methods. Hermes is sync end-to-end.
//!
//! 6. **Pluggable backend.** [`SessionStore::with_memory`] accepts
//!    any `Arc<SurrealMemory>`, so embedded / single-node TCP /
//!    TiKV-clustered deployments use the same store API. Hermes is
//!    SQLite-only.
//!
//! ## Hermes-equivalent capabilities, all preserved
//!
//! - `create_session(surface, model)` → fresh [`Session`].
//! - `append_turn(session_id, parent_id, role, content, taint)` →
//!   the turn plus the new chain head.
//! - `get_session(id)`, `list_recent_sessions(limit)`.
//! - `get_turn(id)`, `list_session_turns(session_id)`.
//! - `lineage_to_root(turn_id)` (Hermes parity: child→root walk).
//! - `lineage_children(turn_id)` (Hermes parity: parent→children).
//! - `fts_search(query, limit)` (Hermes parity, BM25).
//! - **New:** `vector_search(query, k)`, `hybrid_search(query, k,
//!   alpha)`, `lineage_edge(child_id)`, `chain_head()`,
//!   `verify_chain()`.
//!
//! Phase 2 follow-on slices add: TSA anchor (slice 2), dual-write
//! parity diff against a Hermes SQLite import (slice 3), wire into
//! `gaussclaw-agent::TurnPolicy` (slice 4), `gaussclaw-web` /
//! `gaussclaw-surfaces` integration so `/api/sessions` and
//! `/api/receipt/head` return live data (slice 5).

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::significant_drop_tightening,
)]

pub mod embed;
pub mod store;
pub mod types;

pub use embed::{EMBED_DIM, mock_embed};
pub use store::{SessionStore, StoreError, StoreResult};
pub use types::{ChainHead, LineageEdge, Session, Turn, TurnCost, TurnHit, now_rfc3339};

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::TaintLabel;

    async fn fresh_store() -> SessionStore {
        SessionStore::open_in_memory().await.expect("open store")
    }

    // ── sessions ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_session_round_trips() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "anthropic/claude-3.5-sonnet").await;
        assert_eq!(sess.surface, "tui");
        let back = s.get_session(&sess.id).await.expect("get_session");
        assert_eq!(back, sess);
    }

    #[tokio::test]
    async fn list_recent_sessions_newest_first() {
        let s = fresh_store().await;
        let a = s.create_session("tui", "m").await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let b = s.create_session("rest", "m").await;
        let recent = s.list_recent_sessions(10).await;
        assert_eq!(recent.len(), 2);
        // `b` was created after `a`; newest-first ordering puts `b` first.
        assert_eq!(recent[0].id, b.id);
        assert_eq!(recent[1].id, a.id);
    }

    // ── turns + chain ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn append_turn_advances_chain_head() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let head_before = s.chain_head().await.unwrap();
        let (_t, head_after) = s
            .append_turn(&sess.id, None, "user", "hello", TaintLabel::User)
            .await
            .unwrap();
        assert_ne!(head_before.digest_hex, head_after.digest_hex);
        assert_eq!(head_after.length, head_before.length + 1);
    }

    #[tokio::test]
    async fn append_turn_rejects_unknown_session() {
        let s = fresh_store().await;
        let err = s
            .append_turn("nope", None, "user", "hi", TaintLabel::User)
            .await
            .unwrap_err();
        assert!(matches!(err, StoreError::UnknownSession(_)));
    }

    #[tokio::test]
    async fn session_turn_count_advances() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        for i in 0..3 {
            s.append_turn(&sess.id, None, "user", format!("turn-{i}"), TaintLabel::User)
                .await
                .unwrap();
        }
        let updated = s.get_session(&sess.id).await.unwrap();
        assert_eq!(updated.turn_count, 3);
        assert_eq!(s.list_session_turns(&sess.id).await.len(), 3);
    }

    // ── lineage ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn lineage_to_root_walks_parents() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let (root, _) = s
            .append_turn(&sess.id, None, "user", "Q", TaintLabel::User)
            .await
            .unwrap();
        let (mid, _) = s
            .append_turn(&sess.id, Some(root.id), "assistant", "A1", TaintLabel::User)
            .await
            .unwrap();
        let (leaf, _) = s
            .append_turn(&sess.id, Some(mid.id), "user", "Q2", TaintLabel::User)
            .await
            .unwrap();
        let walk = s.lineage_to_root(leaf.id).await;
        let ids: Vec<u64> = walk.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![leaf.id, mid.id, root.id]);
    }

    #[tokio::test]
    async fn lineage_children_returns_immediate() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let (root, _) = s
            .append_turn(&sess.id, None, "user", "Q", TaintLabel::User)
            .await
            .unwrap();
        let (c1, _) = s
            .append_turn(&sess.id, Some(root.id), "assistant", "a", TaintLabel::User)
            .await
            .unwrap();
        let (c2, _) = s
            .append_turn(&sess.id, Some(root.id), "assistant", "b", TaintLabel::User)
            .await
            .unwrap();
        let kids = s.lineage_children(root.id).await;
        let mut ids: Vec<u64> = kids.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![c1.id, c2.id]);
    }

    #[tokio::test]
    async fn lineage_edge_has_blake3_commit_unsigned() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let (root, _) = s
            .append_turn(&sess.id, None, "user", "Q", TaintLabel::User)
            .await
            .unwrap();
        let (child, _) = s
            .append_turn(&sess.id, Some(root.id), "assistant", "A", TaintLabel::User)
            .await
            .unwrap();
        let edge = s.lineage_edge(child.id).await.unwrap();
        assert_eq!(edge.from, root.id);
        assert_eq!(edge.to, child.id);
        assert_eq!(edge.commit_hex.len(), 64, "BLAKE3 hex = 64 chars");
        // No signer attached → no Ed25519 signature.
        assert!(edge.signature_hex.is_none());
    }

    #[tokio::test]
    async fn lineage_edge_carries_ed25519_signature_when_signed() {
        use ed25519_dalek::Verifier;
        let s = signed_store().await;
        let sess = s.create_session("tui", "m").await;
        let (root, _) = s
            .append_turn(&sess.id, None, "user", "Q", TaintLabel::User)
            .await
            .unwrap();
        let (child, _) = s
            .append_turn(&sess.id, Some(root.id), "assistant", "A", TaintLabel::User)
            .await
            .unwrap();
        let edge = s.lineage_edge(child.id).await.unwrap();
        assert_eq!(edge.commit_hex.len(), 64);
        let sig = edge.signature_hex.expect("Ed25519 signature must be present");
        assert_eq!(sig.len(), 128, "Ed25519 hex = 128 chars");
        // Verify the signature: reconstruct canonical bytes and check
        // against the store's public key.
        let pk_bytes = s.public_key().expect("public key");
        let pk = ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes).expect("vk");
        let sig_bytes: [u8; 64] = (0..64)
            .map(|i| {
                u8::from_str_radix(&sig[i * 2..i * 2 + 2], 16).expect("hex")
            })
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap();
        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        // Reconstruct canonical bytes the store signed over.
        let head = s.chain_head().await.unwrap();
        let mut head_bytes = [0u8; 32];
        for (i, slot) in head_bytes.iter_mut().enumerate() {
            *slot =
                u8::from_str_radix(&head.digest_hex[i * 2..i * 2 + 2], 16).expect("hex");
        }
        let mut canonical = Vec::new();
        canonical.extend_from_slice(&root.id.to_le_bytes());
        canonical.extend_from_slice(&child.id.to_le_bytes());
        canonical.extend_from_slice(&head_bytes);
        pk.verify(&canonical, &signature)
            .expect("Ed25519 verification must succeed");
    }

    // ── search ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fts_search_finds_a_turn() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        s.append_turn(&sess.id, None, "user", "the quick brown fox", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "user", "lazy dog jumps", TaintLabel::User)
            .await
            .unwrap();
        let hits = s.fts_search("fox", 10).await.unwrap();
        assert!(!hits.is_empty(), "FTS must find the fox turn");
        assert!(hits[0].turn.content.contains("fox"));
    }

    #[tokio::test]
    async fn vector_search_returns_materialisable_hits() {
        // Forward contract: vector_search returns Vec<TurnHit> without
        // errors and every hit corresponds to a real Turn in the store.
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        s.append_turn(&sess.id, None, "user", "alpha", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "user", "beta", TaintLabel::User)
            .await
            .unwrap();
        let hits = s.vector_search("alpha", 5).await.unwrap();
        for h in &hits {
            assert!(s.get_turn(h.turn.id).await.is_some());
        }
    }

    #[tokio::test]
    async fn hybrid_search_exact_match_recalls_target_turn() {
        // Stronger HNSW + BM25 union check: an exact text match must
        // appear in the hybrid result. FTS alone is sufficient for
        // recall here; we just verify the union does not lose hits.
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let bodies = ["unique_marker_alpha", "filler beta", "filler gamma"];
        let mut target_id = 0u64;
        for body in bodies {
            let (t, _) = s
                .append_turn(&sess.id, None, "user", body, TaintLabel::User)
                .await
                .unwrap();
            if body.contains("unique_marker_alpha") {
                target_id = t.id;
            }
        }
        // alpha=1.0 → FTS-only weight; alpha=0.0 → vector-only.
        // alpha=0.5 → equal merge. The unique-marker text guarantees
        // the BM25 channel ranks the target first.
        let hits = s.hybrid_search("unique_marker_alpha", 5, 0.7).await.unwrap();
        assert!(!hits.is_empty(), "exact-marker hybrid must produce hits");
        let found = hits.iter().any(|h| h.turn.id == target_id);
        assert!(
            found,
            "hybrid recall must contain the exact-match turn id {target_id}"
        );
    }

    #[tokio::test]
    async fn hybrid_search_combines_channels() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        for body in ["alpha alpha alpha", "beta", "alpha beta"] {
            s.append_turn(&sess.id, None, "user", body, TaintLabel::User)
                .await
                .unwrap();
        }
        let hits = s.hybrid_search("alpha", 3, 0.5).await.unwrap();
        assert!(!hits.is_empty(), "hybrid recall returned no hits");
    }

    // ── tamper-evidence ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn verify_chain_passes_on_clean_store() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        for i in 0..5 {
            s.append_turn(&sess.id, None, "user", format!("t{i}"), TaintLabel::User)
                .await
                .unwrap();
        }
        s.verify_chain().await.expect("clean store must verify");
    }

    // ── signed receipts (Ed25519, EUF-CMA) ──────────────────────────────────

    async fn signed_store() -> SessionStore {
        use std::sync::Arc;
        use gauss_audit::{Ed25519Signer, ReceiptSigner};
        let signer = Arc::new(ReceiptSigner::new(
            Ed25519Signer::from_seed([0x42; 32]),
        ));
        SessionStore::open_in_memory()
            .await
            .unwrap()
            .with_signer(signer)
    }

    #[tokio::test]
    async fn signer_attaches_a_public_key() {
        let s = signed_store().await;
        assert!(s.public_key().is_some());
        // Length is 32 (Ed25519 public key).
        let pk = s.public_key().unwrap();
        assert_eq!(pk.len(), 32);
        // Unsigned store has no public key.
        let u = fresh_store().await;
        assert!(u.public_key().is_none());
    }

    #[tokio::test]
    async fn signed_receipt_round_trips() {
        let s = signed_store().await;
        let sess = s.create_session("tui", "m").await;
        let (turn, _head) = s
            .append_turn(&sess.id, None, "user", "signed", TaintLabel::User)
            .await
            .unwrap();
        let receipt = s.get_receipt(turn.id).await.expect("receipt present");
        assert_eq!(u64::try_from(receipt.turn_id.as_u128()).unwrap(), turn.id);
        // Verification against the stored payload succeeds.
        assert!(s.verify_receipt(turn.id).await.unwrap());
    }

    #[tokio::test]
    async fn unsigned_store_has_no_receipt() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let (t, _) = s
            .append_turn(&sess.id, None, "user", "x", TaintLabel::User)
            .await
            .unwrap();
        assert!(s.get_receipt(t.id).await.is_none());
        // verify_receipt returns false (no receipt to verify), not Err.
        assert!(!s.verify_receipt(t.id).await.unwrap());
    }

    #[tokio::test]
    async fn tampered_payload_fails_signature_verify() {
        let s = signed_store().await;
        let sess = s.create_session("tui", "m").await;
        let (t, _) = s
            .append_turn(&sess.id, None, "user", "original", TaintLabel::User)
            .await
            .unwrap();
        // Tamper with the stored payload — the signature should fail
        // because the digest no longer matches.
        {
            let mut st = s.state.lock().await;
            if let Some(p) = st.receipt_payloads.get_mut(&t.id) {
                p[0] = p[0].wrapping_add(1);
            }
        }
        let err = s.verify_receipt(t.id).await.unwrap_err();
        // `gauss_audit::SignedReceipt::verify` returns
        // `GaussError::SignatureInvalid` on mismatch.
        assert!(
            matches!(err, StoreError::Backend(_)),
            "expected Backend(SignatureInvalid), got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_chain_catches_in_memory_tamper() {
        let s = fresh_store().await;
        let sess = s.create_session("tui", "m").await;
        let (t, _) = s
            .append_turn(&sess.id, None, "user", "original", TaintLabel::User)
            .await
            .unwrap();
        // Tamper with the in-memory mirror — corrupt the content of an
        // existing turn. The chain replay must catch it.
        {
            let mut st = s.state.lock().await;
            if let Some(entry) = st.turns.get_mut(&t.id) {
                entry.content = "tampered".into();
            }
        }
        let err = s.verify_chain().await.unwrap_err();
        assert!(matches!(err, StoreError::ChainDivergence { .. }));
    }
}
