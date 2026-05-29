//! Local, dependency-free text embedding via feature hashing.
//!
//! Vector recall needs a `text → Vec<f32>` map with **lexical
//! locality**: texts that share words should land near each other under
//! cosine similarity. The original placeholder hashed the whole string
//! through BLAKE3, which gave a stable vector but *no* locality — two
//! near-identical sentences produced unrelated vectors, so HNSW vector
//! search was effectively random.
//!
//! [`embed`] replaces that with the well-known **hashing trick**
//! (feature hashing): tokenise into lowercase word tokens, hash each
//! token to a signed coordinate in the embedding space, accumulate term
//! frequencies, then L2-normalise. Documents sharing tokens now have
//! genuinely higher cosine similarity. It is deterministic,
//! offline/CI-safe, and needs no model file or heavyweight runtime.
//!
//! This is a *lexical* embedding, not a neural semantic one — synonyms
//! that share no tokens stay apart. A real semantic model (e.g. a
//! MiniLM ONNX checkpoint) is an additive drop-in at this same
//! `text → Vec<f32>` boundary; it isn't bundled here because it would
//! require a network model download and a large inference runtime that
//! don't fit an offline build.

use blake3::Hasher;

/// HNSW dimension expected by `gauss_memory::SurrealMemory`.
pub const EMBED_DIM: usize = 384;

/// Split `text` into lowercase alphanumeric word tokens.
fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
}

/// Hash a token to `(dimension index, sign)` using BLAKE3. The sign bit
/// (signed feature hashing) reduces the bias collisions would otherwise
/// introduce into the accumulated coordinates.
fn hash_token(token: &str) -> (usize, f32) {
    let h = Hasher::new().update(token.as_bytes()).finalize();
    let b = h.as_bytes();
    let idx = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) % EMBED_DIM;
    let sign = if b[4] & 1 == 0 { 1.0 } else { -1.0 };
    (idx, sign)
}

/// Build a deterministic, L2-normalised 384-dim feature-hashing
/// embedding for `text`.
///
/// Identical text always produces an identical vector; texts that share
/// tokens have higher cosine similarity. Empty / token-free text maps to
/// the zero vector.
#[must_use]
pub fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; EMBED_DIM];
    for token in tokenize(text) {
        let (idx, sign) = hash_token(&token);
        v[idx] += sign; // term-frequency weighted (accumulates on repeats)
    }
    // L2-normalise so cosine similarity is scale-free. Leave the zero
    // vector untouched when there are no tokens.
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Backwards-compatible alias for [`embed`].
///
/// Retained so existing call sites and docs that reference `mock_embed`
/// keep working; new code should call [`embed`].
#[must_use]
pub fn mock_embed(text: &str) -> Vec<f32> {
    embed(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn dimension_matches_hnsw_expectation() {
        assert_eq!(embed("hello").len(), EMBED_DIM);
    }

    #[test]
    fn embedding_is_deterministic() {
        assert_eq!(embed("the quick brown fox"), embed("the quick brown fox"));
    }

    #[test]
    fn different_text_produces_different_embeddings() {
        assert_ne!(embed("alpha"), embed("beta"));
    }

    #[test]
    fn normalised_vectors_are_unit_length() {
        let v = embed("some words here");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn empty_text_is_the_zero_vector() {
        let v = embed("   !!!  ");
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn shared_tokens_raise_cosine_similarity() {
        // Two sentences sharing most words must be closer than two that
        // share none — the property the hashing trick buys us and the
        // whole-string hash did not.
        let a = embed("deploy the production cluster tonight");
        let b = embed("deploy the production cluster tomorrow");
        let c = embed("a completely unrelated sentence about cats");
        let ab = cosine(&a, &b);
        let ac = cosine(&a, &c);
        assert!(
            ab > ac,
            "expected shared-token similarity {ab} > unrelated {ac}"
        );
        assert!(
            ab > 0.5,
            "near-identical sentences should be very close: {ab}"
        );
    }

    #[test]
    fn tokenization_is_case_insensitive() {
        assert_eq!(embed("Hello World"), embed("hello world"));
    }
}
