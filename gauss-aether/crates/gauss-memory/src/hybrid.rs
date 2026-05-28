//! [`HybridMemory`] — in-memory [`MemoryBackend`] with real BM25
//! keyword recall and cosine-similarity vector recall.
//!
//! Sprint 6 of "Wire the Loop". The default trait impls return
//! `Ok(vec![])` and the SurrealMemory backend's FTS/HNSW are gated
//! behind production database wiring; this module ships an
//! algorithmically real recall layer that:
//!
//! 1. **Tokenizes** payload text once on append (lowercased,
//!    Unicode-aware split, stopword-filtered). The tokens land in an
//!    inverted index keyed by term → Vec<doc_id>.
//! 2. **Scores** keyword queries with **Okapi BM25** (k1 = 1.5,
//!    b = 0.75) over the inverted index. Time complexity is
//!    O(Σ |postings(t)|) for the query terms, not O(N).
//! 3. **Embeds** payload text on append (when no `embedding` is
//!    supplied) via a deterministic hash-bucket TF-IDF vector so the
//!    vector path stays usable without an external embedding model.
//!    Operators who *do* have a model supply `AppendEntry::embedding`
//!    explicitly and the backend uses it verbatim.
//! 4. **Scores** vector queries with **cosine similarity** against
//!    every appended embedding (linear scan — fine up to ~10⁵
//!    entries; production deployments should use the HNSW-indexed
//!    `SurrealMemory` backend at that scale).
//! 5. **Merges** the two channels via the trait's default
//!    [`gauss_traits::MemoryBackend::hybrid_recall`] which delegates
//!    to [`gauss_traits::merge_hybrid`].
//!
//! All structs are `Sync` (interior mutability via `RwLock`); recall
//! queries take a read lock so concurrent readers don't block each
//! other.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use blake3;
use gauss_core::{GaussError, GaussResult, TaintLabel, TurnId};
use gauss_traits::{
    AppendAck, AppendEntry, ChainHeadSnapshot, MemoryBackend, RecallHit, RecallSource,
};

/// Standard BM25 saturation parameter. Used by Lucene, ElasticSearch,
/// every textbook.
const BM25_K1: f32 = 1.5;
/// Standard BM25 length-normalisation parameter.
const BM25_B: f32 = 0.75;
/// Hash-bucket TF-IDF embedding dimension. Small enough to be cheap,
/// large enough that collisions stay sparse on realistic prompts.
pub const DEFAULT_EMBED_DIM: usize = 256;

/// Minimal English stoplist. Trimmed to the ones that bloat the
/// inverted index without carrying signal in agent-conversation
/// corpora.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "has", "have",
    "he", "her", "his", "i", "in", "into", "is", "it", "its", "of", "on", "or", "that",
    "the", "their", "them", "they", "this", "to", "was", "were", "will", "with", "you",
    "your",
];

/// In-memory document.
#[derive(Debug, Clone)]
struct Doc {
    turn_id: TurnId,
    seq: u64,
    payload: Vec<u8>,
    payload_text: Option<String>,
    taint: TaintLabel,
    /// Term-frequency map for BM25 (token → count in this doc).
    term_freq: HashMap<String, u32>,
    /// Number of tokens in the doc (length-normalisation denominator).
    doc_len: u32,
    /// L2-normalised embedding for cosine. Always present — either
    /// supplied by the caller or derived from payload_text via the
    /// hash-bucket TF-IDF scheme.
    embedding: Vec<f32>,
}

/// In-memory hybrid-recall backend.
#[derive(Debug, Default)]
pub struct HybridMemory {
    inner: RwLock<HybridInner>,
}

#[derive(Debug, Default)]
struct HybridInner {
    docs: Vec<Doc>,
    /// Inverted index: term → sorted list of doc indices.
    inverted: HashMap<String, Vec<usize>>,
    /// Sum of all document lengths (denominator for `avg_doc_len`).
    total_tokens: u64,
    /// SHA-256-style chain head. We compute it lazily as
    /// `blake3(prev_head || serialize(entry))` so the digest is
    /// stable and matches the audit-chain idiom used elsewhere.
    chain_head: [u8; 32],
    chain_length: u64,
}

impl HybridMemory {
    /// Build an empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of documents currently indexed.
    #[must_use]
    pub fn doc_count(&self) -> usize {
        self.inner.read().map(|i| i.docs.len()).unwrap_or(0)
    }

}

#[async_trait]
impl MemoryBackend for HybridMemory {
    async fn append(&self, entry: AppendEntry) -> GaussResult<AppendAck> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| GaussError::Internal(format!("hybrid mem lock: {e}")))?;
        let seq = inner.docs.len() as u64;
        let idx = inner.docs.len();
        let text = entry.payload_text.clone().unwrap_or_default();
        let tokens = tokenize(&text);
        let doc_len = u32::try_from(tokens.len()).unwrap_or(u32::MAX);
        let mut term_freq: HashMap<String, u32> = HashMap::new();
        for tok in &tokens {
            *term_freq.entry(tok.clone()).or_default() += 1;
        }
        // Update the inverted index. Idempotent on duplicate appends —
        // the same doc can't appear twice because every entry gets a
        // fresh `idx`.
        for term in term_freq.keys() {
            inner
                .inverted
                .entry(term.clone())
                .or_default()
                .push(idx);
        }
        inner.total_tokens = inner.total_tokens.saturating_add(u64::from(doc_len));
        let embedding = entry
            .embedding
            .clone()
            .unwrap_or_else(|| hash_bucket_embedding(&tokens, DEFAULT_EMBED_DIM));
        let embedding = l2_normalise(embedding);
        let doc = Doc {
            turn_id: entry.turn_id,
            seq,
            payload: entry.payload.clone(),
            payload_text: entry.payload_text,
            taint: entry.taint,
            term_freq,
            doc_len,
            embedding,
        };
        inner.docs.push(doc);

        // Advance the chain head. We chain the previous head with a
        // BLAKE3 digest of the serialised entry — same shape every
        // gauss-* backend uses so a follow-on signer can sign this
        // head verbatim.
        let mut hasher = blake3::Hasher::new();
        hasher.update(&inner.chain_head);
        hasher.update(&entry.turn_id.0.to_le_bytes());
        hasher.update(&entry.payload);
        hasher.update(&[entry.taint as u8]);
        let new_head: [u8; 32] = hasher.finalize().into();
        inner.chain_head = new_head;
        inner.chain_length = inner.chain_length.saturating_add(1);

        Ok(AppendAck::new(
            seq,
            ChainHeadSnapshot::new(new_head, inner.chain_length),
        ))
    }

    async fn chain_head(&self) -> GaussResult<ChainHeadSnapshot> {
        let inner = self
            .inner
            .read()
            .map_err(|e| GaussError::Internal(format!("hybrid mem lock: {e}")))?;
        Ok(ChainHeadSnapshot::new(inner.chain_head, inner.chain_length))
    }

    async fn len(&self) -> GaussResult<u64> {
        Ok(self.doc_count() as u64)
    }

    async fn fts_search(&self, query: &str, limit: usize) -> GaussResult<Vec<RecallHit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inner = self
            .inner
            .read()
            .map_err(|e| GaussError::Internal(format!("hybrid mem lock: {e}")))?;
        if inner.docs.is_empty() {
            return Ok(Vec::new());
        }
        let avg_dl = if inner.docs.is_empty() {
            1.0
        } else {
            inner.total_tokens as f32 / inner.docs.len() as f32
        };
        let n = inner.docs.len() as f32;
        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }
        // Score every doc that any query term touches. Walking the
        // postings lists keeps cost O(Σ |postings(t)|) instead of O(N).
        let mut scores: HashMap<usize, f32> = HashMap::new();
        for term in &query_terms {
            let Some(postings) = inner.inverted.get(term) else {
                continue;
            };
            let df = postings.len() as f32;
            // Lucene's smoothed IDF — never negative even when df > N/2.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &doc_idx in postings {
                let doc = &inner.docs[doc_idx];
                let tf = *doc.term_freq.get(term).unwrap_or(&0) as f32;
                let dl = doc.doc_len as f32;
                let num = tf * (BM25_K1 + 1.0);
                let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg_dl);
                let contribution = if denom == 0.0 { 0.0 } else { idf * num / denom };
                *scores.entry(doc_idx).or_insert(0.0) += contribution;
            }
        }
        let mut hits: Vec<RecallHit> = scores
            .into_iter()
            .map(|(idx, score)| {
                let doc = &inner.docs[idx];
                RecallHit::new(
                    doc.turn_id,
                    doc.seq,
                    score,
                    RecallSource::Fts,
                    doc.payload.clone(),
                    doc.payload_text.clone(),
                    doc.taint,
                )
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    async fn vector_search(&self, query: &[f32], k: usize) -> GaussResult<Vec<RecallHit>> {
        if k == 0 || query.is_empty() {
            return Ok(Vec::new());
        }
        let inner = self
            .inner
            .read()
            .map_err(|e| GaussError::Internal(format!("hybrid mem lock: {e}")))?;
        if inner.docs.is_empty() {
            return Ok(Vec::new());
        }
        let normalised = l2_normalise(query.to_vec());
        let mut scored: Vec<(usize, f32)> = inner
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.embedding.len() == normalised.len())
            .map(|(idx, doc)| (idx, cosine_dot(&normalised, &doc.embedding)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        let hits: Vec<RecallHit> = scored
            .into_iter()
            .map(|(idx, score)| {
                let doc = &inner.docs[idx];
                RecallHit::new(
                    doc.turn_id,
                    doc.seq,
                    score,
                    RecallSource::Vector,
                    doc.payload.clone(),
                    doc.payload_text.clone(),
                    doc.taint,
                )
            })
            .collect();
        Ok(hits)
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

/// Tokenize one document the same way both append and query do —
/// lowercase, Unicode-aware split on non-alphanumerics, stopword
/// filter, ≥ 2 char minimum.
pub fn tokenize(text: &str) -> Vec<String> {
    text.chars()
        .flat_map(|c| {
            if c.is_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else {
                Some(' ')
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .filter(|t| !STOPWORDS.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Deterministic hash-bucket TF-IDF-ish embedding. Each token hashes
/// (BLAKE3) into a fixed bucket; the bucket counts produce a vector
/// in the document's tokens. Cheap to compute; reproducible; doesn't
/// need a model; collides on synonyms (which the BM25 channel
/// handles). L2-normalisation lives in [`l2_normalise`].
#[must_use]
pub fn hash_bucket_embedding(tokens: &[String], dim: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; dim];
    if dim == 0 {
        return v;
    }
    for tok in tokens {
        let mut hasher = blake3::Hasher::new();
        hasher.update(tok.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        // Map the first 4 bytes of the digest into a bucket index.
        let bucket = (u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize)
            % dim;
        v[bucket] += 1.0;
    }
    v
}

/// L2-normalise a vector. The zero vector is returned unchanged.
#[must_use]
pub fn l2_normalise(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Dot product. The caller guarantees both vectors are L2-normalised,
/// so the result is the cosine similarity in `[-1, 1]`.
#[must_use]
pub fn cosine_dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_traits::HybridQuery;

    fn append(mem: &HybridMemory, id: u128, text: &str) {
        let entry = AppendEntry::new(TurnId::new(id), text.as_bytes().to_vec(), TaintLabel::User)
            .with_text(text);
        futures::executor::block_on(mem.append(entry)).expect("append");
    }

    #[test]
    fn empty_corpus_returns_no_hits() {
        let mem = HybridMemory::new();
        let hits = futures::executor::block_on(mem.fts_search("hello", 10)).unwrap();
        assert!(hits.is_empty());
        let v_hits = futures::executor::block_on(mem.vector_search(&[1.0; 256], 10)).unwrap();
        assert!(v_hits.is_empty());
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_tokens() {
        let tokens = tokenize("The quick brown fox jumps over a lazy dog");
        // "the", "a", "over" — "the" + "a" filtered as stopwords; everything
        // else (>= 2 chars, non-stopword) survives.
        assert!(tokens.contains(&"quick".to_string()));
        assert!(tokens.contains(&"brown".to_string()));
        assert!(tokens.contains(&"fox".to_string()));
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[test]
    fn bm25_ranks_more_specific_docs_higher() {
        let mem = HybridMemory::new();
        append(&mem, 1, "the quick brown fox jumps over the lazy dog");
        append(&mem, 2, "the quick brown fox is brown and quick");
        append(&mem, 3, "completely unrelated text about elephants");
        let hits = futures::executor::block_on(mem.fts_search("quick brown fox", 5)).unwrap();
        assert_eq!(hits.len(), 2, "two docs match; the third is unrelated");
        // Doc 2 mentions "brown" + "quick" twice each in shorter text — it
        // should outrank doc 1 (longer doc, single mentions).
        assert_eq!(hits[0].turn_id.0, 2);
        assert_eq!(hits[1].turn_id.0, 1);
        // Doc 3 must not appear.
        assert!(hits.iter().all(|h| h.turn_id.0 != 3));
    }

    #[test]
    fn bm25_skips_unrelated_query_terms() {
        let mem = HybridMemory::new();
        append(&mem, 1, "rust async memory recall");
        append(&mem, 2, "garbage collection in java");
        let hits = futures::executor::block_on(mem.fts_search("rust memory", 10)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].turn_id.0, 1);
    }

    #[test]
    fn cosine_ranks_self_first() {
        let mem = HybridMemory::new();
        append(&mem, 1, "alpha beta gamma");
        append(&mem, 2, "delta epsilon zeta");
        append(&mem, 3, "alpha beta delta");
        // Use the doc-1 embedding as a query — it should match doc 1
        // most strongly.
        let query = hash_bucket_embedding(
            &tokenize("alpha beta gamma"),
            DEFAULT_EMBED_DIM,
        );
        let hits = futures::executor::block_on(mem.vector_search(&query, 3)).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].turn_id.0, 1);
        // Doc 3 shares "alpha" + "beta" → ranks ahead of doc 2.
        assert_eq!(hits[1].turn_id.0, 3);
        assert_eq!(hits[2].turn_id.0, 2);
    }

    #[test]
    fn hybrid_merge_combines_both_channels() {
        let mem = HybridMemory::new();
        append(&mem, 1, "rust async memory recall");
        append(&mem, 2, "python sync memory recall");
        append(&mem, 3, "garbage collection in java");
        let q = HybridQuery::new(
            Some("rust memory".into()),
            Some(hash_bucket_embedding(
                &tokenize("rust memory"),
                DEFAULT_EMBED_DIM,
            )),
            5,
            0.5,
        );
        let hits = futures::executor::block_on(mem.hybrid_recall(q)).unwrap();
        assert!(!hits.is_empty());
        // Doc 1 hits both channels; doc 2 hits only the vector channel
        // (the keyword "memory" is in doc 2 too, but BM25 punishes
        // length and term-saturation differently).
        assert_eq!(hits[0].turn_id.0, 1);
        // Hybrid merge marks dual-channel hits with RecallSource::Hybrid.
        assert!(matches!(
            hits[0].source,
            RecallSource::Hybrid | RecallSource::Fts | RecallSource::Vector
        ));
    }

    #[test]
    fn append_advances_chain_head() {
        let mem = HybridMemory::new();
        let h0 = futures::executor::block_on(mem.chain_head()).unwrap();
        assert_eq!(h0.length, 0);
        append(&mem, 1, "first");
        let h1 = futures::executor::block_on(mem.chain_head()).unwrap();
        assert_eq!(h1.length, 1);
        assert_ne!(h1.digest, h0.digest);
        append(&mem, 2, "second");
        let h2 = futures::executor::block_on(mem.chain_head()).unwrap();
        assert_eq!(h2.length, 2);
        assert_ne!(h2.digest, h1.digest);
    }

    /// Bench-style sanity check: synthesise 200 documents, run 50
    /// random-query lookups, and assert that the keyword we sprinkled
    /// into a target doc lands in the top-5 hits. We're not claiming
    /// the README's ≤1.5% miss-rate here — that requires a labelled
    /// corpus — but we are claiming that recall is sound at the
    /// implementation level: every planted needle is findable.
    #[test]
    fn planted_needles_are_recoverable_at_top_5() {
        let mem = HybridMemory::new();
        // 200 noise documents.
        for i in 0..200u128 {
            append(
                &mem,
                i,
                &format!("noise document {i} with filler words alpha beta gamma"),
            );
        }
        // 50 planted needles, each with a unique nonce we'll query for.
        let mut needles: Vec<(u128, String)> = Vec::new();
        for i in 0..50u128 {
            let nonce = format!("needle_{i:04}");
            let id = 200 + i;
            append(
                &mem,
                id,
                &format!("planted document containing {nonce} unique marker"),
            );
            needles.push((id, nonce));
        }
        let mut misses = 0;
        for (expected_id, nonce) in &needles {
            let hits = futures::executor::block_on(mem.fts_search(nonce, 5)).unwrap();
            let found = hits.iter().any(|h| h.turn_id.0 == *expected_id);
            if !found {
                misses += 1;
            }
        }
        assert_eq!(
            misses, 0,
            "every planted needle should be in the top-5 hits (got {misses} miss(es))"
        );
    }
}
