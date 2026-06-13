//! DualRAG retrieval orchestration (paper §IV.D, Algorithm 2).
//!
//! For each query DualRAG surfaces both semantically similar items (the
//! **vector path**, an HNSW k-NN query) and structurally connected premise
//! sets (the **graph path**, a typed beam search), then fuses them with
//! reciprocal-rank fusion and packs premises first. The graph path is what
//! lets the composition rule `⊕` fire across model boundaries; the whole
//! pipeline supplies the retrieval-sufficiency factor `r_L` of Lemma 1.
//!
//! This module composes the [`crate::kg::KnowledgeStore`] retrieval primitives
//! with the [`crate::fusion`] stage into the single `retrieve` call the RSI
//! Loop Engine (Phase 5) makes per query. It is generic over the store, so the
//! in-memory reference and the live `gauss-memory` backend drive it
//! identically.

use serde::{Deserialize, Serialize};

use crate::fusion::{pack_premises_first, reciprocal_rank_fusion, RankedList, DEFAULT_RRF_K};
use crate::kg::{ConceptId, KnowledgeStore};
use crate::state::ClaimId;

/// DualRAG retrieval parameters (paper Algorithm 2 inputs `kv, ef, b, L, kf`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DualRagParams {
    /// `kv` — vector-path top-k.
    pub k_vector: usize,
    /// `b` — graph-path beam width.
    pub beam: usize,
    /// `L` — graph-path traversal depth.
    pub depth: usize,
    /// `kf` — final fused context size.
    pub fused_size: usize,
    /// RRF smoothing constant `k` ([`DEFAULT_RRF_K`] is the paper's value).
    pub rrf_k: f64,
}

impl Default for DualRagParams {
    fn default() -> Self {
        Self {
            k_vector: 8,
            beam: 8,
            depth: 2,
            fused_size: 16,
            rrf_k: DEFAULT_RRF_K,
        }
    }
}

/// The packed context returned by DualRAG (Algorithm 2 output): premises
/// (graph-path items) first, then similar items, with provenance attached
/// downstream by the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PackedContext {
    /// The fused, premises-first claim ids.
    pub items: Vec<ClaimId>,
    /// How many leading items came from the graph path (the premise prefix).
    pub premise_count: usize,
}

/// Run DualRAG for one query: vector path (k-NN over `qvec`) + graph path
/// (beam search from `seeds`) → RRF fusion → premises-first packing.
///
/// Returns the [`PackedContext`]; the count of premise (graph-path) items is
/// the part Theorem 2's composition rule relies on.
#[must_use]
pub fn retrieve<S: KnowledgeStore>(
    store: &S,
    qvec: &[f32],
    seeds: &[ConceptId],
    params: &DualRagParams,
) -> PackedContext {
    // Vector path: HNSW k-NN over verified claims (Algorithm 2 line 2).
    let vector: Vec<ClaimId> = store.knn(qvec, params.k_vector);

    // Graph path: typed beam search over premise edges (Algorithm 2 lines 3-6).
    // Flatten the ranked paths into a best-first claim list, de-duplicated,
    // preserving the order the beam produced.
    let mut graph: Vec<ClaimId> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in store.beam(seeds, params.beam, params.depth) {
        for c in path.claims {
            if seen.insert(c) {
                graph.push(c);
            }
        }
    }

    // Fusion: reciprocal-rank fusion of the two rankings (Algorithm 2 lines 7-9).
    let fused = reciprocal_rank_fusion(
        &[RankedList::new(&vector), RankedList::new(&graph)],
        params.rrf_k,
    );
    let fused_ids: Vec<ClaimId> = fused.into_iter().map(|(id, _)| id).collect();

    // Packing: premises (graph path) first, then the remaining fused items
    // (Algorithm 2 line 10). Restrict the "premise" prefix to graph-path items
    // that survived fusion.
    let graph_set: std::collections::BTreeSet<ClaimId> = graph.iter().copied().collect();
    let premises: Vec<ClaimId> = fused_ids
        .iter()
        .copied()
        .filter(|c| graph_set.contains(c))
        .collect();
    let similar: Vec<ClaimId> = fused_ids
        .iter()
        .copied()
        .filter(|c| !graph_set.contains(c))
        .collect();
    let items = pack_premises_first(&premises, &similar, params.fused_size);
    let premise_count = items.iter().filter(|c| graph_set.contains(c)).count();

    PackedContext {
        items,
        premise_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kg::{AdmitBatch, Claim, ClaimStatus, Concept, ModelId, Provenance};

    fn claim(id: u64, embedding: &[f32]) -> Claim {
        Claim {
            id: ClaimId(id),
            content: format!("c{id}"),
            embedding: embedding.to_vec(),
            confidence: 0.9,
            status: ClaimStatus::Verified,
            provenance: Provenance {
                models: vec![ModelId("m".into())],
                model_families: std::iter::once("openai".to_owned()).collect(),
                premises: Vec::new(),
                verifier_tier: 1,
                cycle: 0,
            },
        }
    }

    fn store_with_premise_chain() -> crate::kg::InMemoryKnowledgeStore {
        let mut store = crate::kg::InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![
                claim(1, &[1.0, 0.0]),
                claim(2, &[0.9, 0.1]),
                claim(3, &[0.0, 1.0]),
            ],
            skills: Vec::new(),
            // claim 1 derived from claim 2 (a premise link).
            derived_from: vec![(ClaimId(1), ClaimId(2))],
        });
        store.add_concept(
            Concept {
                id: ConceptId(10),
                name: "seed".into(),
                embedding: vec![1.0, 0.0],
            },
            vec![ClaimId(1)],
        );
        store
    }

    #[test]
    fn retrieve_surfaces_both_paths_and_packs_premises_first() {
        let store = store_with_premise_chain();
        let ctx = retrieve(
            &store,
            &[1.0, 0.0],
            &[ConceptId(10)],
            &DualRagParams::default(),
        );
        // The premise chain (claims 1, 2 from the graph path) must appear, and
        // premise items must lead the packed context.
        assert!(ctx.items.contains(&ClaimId(1)));
        assert!(ctx.items.contains(&ClaimId(2)));
        assert!(ctx.premise_count >= 1);
        // The first `premise_count` items are exactly the graph-path prefix.
        let graph_prefix = &ctx.items[..ctx.premise_count];
        assert!(graph_prefix.contains(&ClaimId(1)) || graph_prefix.contains(&ClaimId(2)));
    }

    #[test]
    fn fused_size_caps_the_context() {
        let store = store_with_premise_chain();
        let params = DualRagParams {
            fused_size: 1,
            ..DualRagParams::default()
        };
        let ctx = retrieve(&store, &[1.0, 0.0], &[ConceptId(10)], &params);
        assert_eq!(ctx.items.len(), 1);
    }

    #[test]
    fn without_graph_path_premise_count_collapses() {
        // NOGRAPH ablation: no seeds => no premises => r_L collapses for
        // multi-premise items (paper §IV.D, Theorem 2(b) loses its engine).
        let store = store_with_premise_chain();
        let ctx = retrieve(&store, &[1.0, 0.0], &[], &DualRagParams::default());
        assert_eq!(ctx.premise_count, 0);
    }
}
