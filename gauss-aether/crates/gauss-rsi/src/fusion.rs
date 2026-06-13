//! DualRAG fusion stage — reciprocal-rank fusion (Algorithm 2).
//!
//! DualRAG surfaces, for each query, both semantically similar items (the
//! vector path, an HNSW k-NN query) and structurally connected premise sets
//! (the graph path, a typed beam search), then *fuses* them. Fusion is
//! reciprocal-rank fusion followed by a cross-encoder re-rank (paper §IV.D):
//!
//! ```text
//! rrf(i) = Σ_{p ∈ {V, G}} 1 / (k + rank_p(i)),
//! ```
//!
//! with the canonical constant `k = 60`. The packed context places premises
//! (graph-path items) first so the composition rule `⊕` can fire across model
//! boundaries; this stage supplies the retrieval-sufficiency factor `r_L` of
//! Lemma 1.

use std::collections::BTreeMap;

/// The canonical RRF constant `k` (paper Listing/Algorithm 2: `1/(60 + rank)`).
pub const DEFAULT_RRF_K: f64 = 60.0;

/// One ranked retrieval path: items in descending relevance order. Rank is
/// the zero-based position, so the top item has rank `0`.
#[derive(Debug, Clone)]
pub struct RankedList<'a, T> {
    items: &'a [T],
}

impl<'a, T> RankedList<'a, T> {
    /// Wrap a slice already sorted best-first.
    #[must_use]
    pub const fn new(items: &'a [T]) -> Self {
        Self { items }
    }
}

/// Reciprocal-rank fusion of several ranked paths.
///
/// Returns the fused items in descending fused-score order. Ties break by the
/// item's natural ordering, so the fusion is deterministic — required by the
/// conformance suite. `k` is the RRF smoothing constant
/// ([`DEFAULT_RRF_K`] is the paper's value).
#[must_use]
pub fn reciprocal_rank_fusion<T>(paths: &[RankedList<'_, T>], k: f64) -> Vec<(T, f64)>
where
    T: Ord + Clone,
{
    let mut scores: BTreeMap<T, f64> = BTreeMap::new();
    for path in paths {
        for (rank, item) in path.items.iter().enumerate() {
            // rank is a small usize index; convert via f64 (no int overflow).
            #[allow(clippy::cast_precision_loss)]
            let contribution = 1.0 / (k + rank as f64);
            scores
                .entry(item.clone())
                .and_modify(|s| *s += contribution)
                .or_insert(contribution);
        }
    }
    let mut fused: Vec<(T, f64)> = scores.into_iter().collect();
    // Sort by score descending; ties already grouped by the BTreeMap's key
    // order, so break ties by key ascending for a total deterministic order.
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}

/// Pack the fused context premises-first (Algorithm 2, line 10): graph-path
/// items (premises) lead, then any vector-path-only items, de-duplicated.
///
/// Returns up to `fused_size` item identifiers in packed order.
#[must_use]
pub fn pack_premises_first<T>(graph_path: &[T], vector_path: &[T], fused_size: usize) -> Vec<T>
where
    T: Ord + Clone,
{
    let mut seen: std::collections::BTreeSet<T> = std::collections::BTreeSet::new();
    let mut packed: Vec<T> = Vec::new();
    for item in graph_path.iter().chain(vector_path.iter()) {
        if packed.len() >= fused_size {
            break;
        }
        if seen.insert(item.clone()) {
            packed.push(item.clone());
        }
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fusion_rewards_items_ranked_highly_in_both_paths() {
        let vector = [10_u32, 20, 30];
        let graph = [20_u32, 40, 10];
        let fused = reciprocal_rank_fusion(
            &[RankedList::new(&vector), RankedList::new(&graph)],
            DEFAULT_RRF_K,
        );
        // 10 appears at rank 0 (vector) and rank 2 (graph); 20 at rank 1 and 0.
        // Both beat the singletons 30 and 40.
        let order: Vec<u32> = fused.iter().map(|(i, _)| *i).collect();
        let pos = |x: u32| order.iter().position(|&y| y == x).unwrap();
        assert!(pos(10) < pos(30));
        assert!(pos(20) < pos(40));
    }

    #[test]
    fn fusion_is_deterministic_on_ties() {
        // Two items each appearing once at rank 0 have equal score; the tie
        // breaks by ascending key, deterministically.
        let a = [5_u32];
        let b = [3_u32];
        let fused =
            reciprocal_rank_fusion(&[RankedList::new(&a), RankedList::new(&b)], DEFAULT_RRF_K);
        assert_eq!(fused[0].0, 3);
        assert_eq!(fused[1].0, 5);
    }

    #[test]
    fn higher_rank_contributes_more() {
        let single = [100_u32, 200];
        let fused = reciprocal_rank_fusion(&[RankedList::new(&single)], DEFAULT_RRF_K);
        // rank 0 score 1/60 > rank 1 score 1/61.
        assert!(fused[0].1 > fused[1].1);
        assert_eq!(fused[0].0, 100);
    }

    #[test]
    fn packing_puts_premises_first_and_dedups() {
        let graph = [1_u32, 2, 3];
        let vector = [3_u32, 4, 5];
        let packed = pack_premises_first(&graph, &vector, 10);
        assert_eq!(packed, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn packing_respects_the_fused_size_cap() {
        let graph = [1_u32, 2, 3];
        let vector = [4_u32, 5, 6];
        let packed = pack_premises_first(&graph, &vector, 2);
        assert_eq!(packed, vec![1, 2]);
    }
}
