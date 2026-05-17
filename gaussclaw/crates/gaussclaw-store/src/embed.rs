//! Deterministic mock embedding.
//!
//! Phase 2 of `GAUSSCLAW_ROADMAP.md` does not have provider drivers
//! wired yet (those land in Phase 4), so HNSW indexing can't use a
//! real semantic embedding. We use a deterministic [`mock_embed`]
//! that maps text bytes through BLAKE3 to a fixed-length `f32` vector
//! in the SurrealMemory HNSW dimension (384). Results are reproducible
//! across runs — tests against vector recall are stable.
//!
//! Real embeddings (Phase 4) plug in by implementing the same
//! `text → Vec<f32>` shape.

use blake3::Hasher;

/// HNSW dimension expected by `gauss_memory::SurrealMemory`.
pub const EMBED_DIM: usize = 384;

/// Build a deterministic 384-dim embedding for `text`.
///
/// Maps the BLAKE3 hash of `text` over 384 `f32` slots in `[0, 1]`.
/// Identical text always produces an identical vector; cosine
/// similarity between two embeddings reflects shared prefixes in the
/// BLAKE3 output, which is enough to make vector-recall tests pass
/// without committing to any specific semantic model.
#[must_use]
pub fn mock_embed(text: &str) -> Vec<f32> {
    let mut out = Vec::with_capacity(EMBED_DIM);
    let mut hasher = Hasher::new();
    hasher.update(text.as_bytes());
    // BLAKE3 supports unbounded output via the XOF; we feed it through
    // a counter-mode loop to get enough bytes for `EMBED_DIM * 4`.
    let mut xof = hasher.finalize_xof();
    let mut buf = [0u8; 4 * EMBED_DIM];
    xof.fill(&mut buf);
    for i in 0..EMBED_DIM {
        let start = i.saturating_mul(4);
        let bytes = [buf[start], buf[start + 1], buf[start + 2], buf[start + 3]];
        // Map a u32 to f32 in [0, 1].
        let n = u32::from_le_bytes(bytes);
        out.push(f32::from(u16::try_from(n >> 16).unwrap_or(0)) / 65535.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_matches_hnsw_expectation() {
        let v = mock_embed("hello");
        assert_eq!(v.len(), EMBED_DIM);
    }

    #[test]
    fn embedding_is_deterministic() {
        let a = mock_embed("the quick brown fox");
        let b = mock_embed("the quick brown fox");
        assert_eq!(a, b, "mock_embed must be deterministic");
    }

    #[test]
    fn different_text_produces_different_embeddings() {
        let a = mock_embed("alpha");
        let b = mock_embed("beta");
        assert_ne!(a, b);
    }

    #[test]
    fn embedding_values_are_bounded() {
        let v = mock_embed("bounded test");
        for x in &v {
            assert!((0.0..=1.0).contains(x), "value out of bounds: {x}");
        }
    }
}
