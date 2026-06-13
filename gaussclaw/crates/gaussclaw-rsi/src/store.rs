//! `SurrealKnowledgeStore` — a live SurrealDB-backed [`AsyncKnowledgeStore`].
//!
//! Implements the Gauss-Agent0 KnowledgeGraph (paper §IV.C, Appendix A) on an
//! embedded SurrealDB instance: claims, skills, concepts, `derived_from`
//! edges, provenance, and per-cycle snapshots all persist in the database, so
//! rollback is a watermark delete and the synergy estimate of Theorem 2(b) is
//! a `GROUP ALL` count.
//!
//! Vector ranking is computed in-process over the DB-fetched embeddings, which
//! keeps the store correct for any embedding dimension; the production index
//! is SurrealDB's HNSW `<|k|>` operator (declared in
//! [`gauss_rsi::SCHEMA_SURREALQL`]).

use async_trait::async_trait;
use gauss_rsi::kg::{AdmitBatch, ConceptId, Path, SnapshotId};
use gauss_rsi::state::ClaimId;
use gauss_rsi::AsyncKnowledgeStore;
use serde::{Deserialize, Serialize};
use surrealdb::engine::local::{Db, Mem};
use surrealdb::Surreal;

/// Error opening or migrating the store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The underlying SurrealDB call failed.
    #[error("surrealdb: {0}")]
    Surreal(#[from] surrealdb::Error),
}

/// A live SurrealDB knowledge store (embedded `Mem` engine).
pub struct SurrealKnowledgeStore {
    db: Surreal<Db>,
}

#[derive(Debug, Serialize)]
struct ClaimRow {
    cid: i64,
    content: String,
    embedding: Vec<f32>,
    confidence: f64,
    status: String,
    cycle: i64,
    families: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EdgeRow {
    child: i64,
    parent: i64,
    cycle: i64,
}

#[derive(Debug, Serialize)]
struct SkillRow {
    cid: i64,
    cycle: i64,
}

#[derive(Debug, Deserialize)]
struct EmbRow {
    cid: i64,
    #[serde(default)]
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct ParentRow {
    parent: i64,
}

#[derive(Debug, Deserialize)]
struct AboutRow {
    #[serde(default)]
    about: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct CountRow {
    count: i64,
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
const fn to_i64(v: u64) -> i64 {
    v as i64
}

#[allow(clippy::cast_sign_loss)]
const fn to_u64(v: i64) -> u64 {
    if v < 0 {
        0
    } else {
        v as u64
    }
}

impl SurrealKnowledgeStore {
    /// Open an embedded in-memory SurrealDB instance for the RSI namespace.
    ///
    /// # Errors
    /// Returns [`StoreError`] if the engine fails to start or the
    /// namespace/database cannot be selected.
    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let db = Surreal::new::<Mem>(()).await?;
        db.use_ns("gauss").use_db("rsi").await?;
        Ok(Self { db })
    }

    /// Cosine similarity over the shared prefix; `0.0` for a zero-norm vector.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0_f32;
        let mut na = 0.0_f32;
        let mut nb = 0.0_f32;
        for (x, y) in a.iter().zip(b.iter()) {
            dot += x * y;
            na += x * x;
            nb += y * y;
        }
        let denom = na.sqrt() * nb.sqrt();
        if denom > 0.0 {
            dot / denom
        } else {
            0.0
        }
    }

    async fn count_where(&self, clause: &str, watermark: i64) -> usize {
        let sql = format!("SELECT count() AS count FROM {clause} GROUP ALL");
        match self.db.query(sql).bind(("w", watermark)).await {
            Ok(mut resp) => match resp.take::<Vec<CountRow>>(0) {
                Ok(rows) => rows
                    .first()
                    .map_or(0, |r| usize::try_from(r.count.max(0)).unwrap_or(0)),
                Err(e) => {
                    tracing::warn!(error = %e, "count deserialize failed");
                    0
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "count query failed");
                0
            }
        }
    }
}

#[async_trait]
impl AsyncKnowledgeStore for SurrealKnowledgeStore {
    async fn admit(&mut self, batch: AdmitBatch) {
        // UPSERT by a content-derived record id so re-admitting an identical
        // item is idempotent (no duplicate rows) — the store reflects the
        // distinct verified state `K`, not the per-cycle admission stream.
        for claim in batch.claims {
            let cid = to_i64(claim.id.0);
            let row = ClaimRow {
                cid,
                content: claim.content,
                embedding: claim.embedding,
                confidence: claim.confidence,
                status: "verified".to_owned(),
                cycle: to_i64(u64::from(claim.provenance.cycle)),
                families: claim.provenance.model_families.into_iter().collect(),
            };
            if let Err(e) = self
                .db
                .query("UPSERT type::thing('claim', $id) CONTENT $r")
                .bind(("id", cid))
                .bind(("r", row))
                .await
            {
                tracing::warn!(error = %e, "claim upsert failed");
            }
        }
        for skill in batch.skills {
            let cid = to_i64(skill.id.0);
            let row = SkillRow {
                cid,
                cycle: to_i64(u64::from(skill.cycle)),
            };
            if let Err(e) = self
                .db
                .query("UPSERT type::thing('skill', $id) CONTENT $r")
                .bind(("id", cid))
                .bind(("r", row))
                .await
            {
                tracing::warn!(error = %e, "skill upsert failed");
            }
        }
        for (child, parent) in batch.derived_from {
            let row = EdgeRow {
                child: to_i64(child.0),
                parent: to_i64(parent.0),
                cycle: 0,
            };
            if let Err(e) = self
                .db
                .query("UPSERT type::thing('derived', [$child, $parent]) CONTENT $r")
                .bind(("child", row.child))
                .bind(("parent", row.parent))
                .bind(("r", row))
                .await
            {
                tracing::warn!(error = %e, "edge upsert failed");
            }
        }
    }

    async fn checkpoint(&mut self, cycle: u32, label: &str) -> SnapshotId {
        let label = label.to_owned();
        if let Err(e) = self
            .db
            .query("CREATE snapshot CONTENT { cycle: $c, label: $l }")
            .bind(("c", i64::from(cycle)))
            .bind(("l", label))
            .await
        {
            tracing::warn!(error = %e, "snapshot insert failed");
        }
        SnapshotId(cycle)
    }

    async fn rollback(&mut self, to: SnapshotId) -> usize {
        let watermark = i64::from(to.0);
        let dropped = self
            .count_where("claim WHERE cycle > $w", watermark)
            .await
            .saturating_add(self.count_where("skill WHERE cycle > $w", watermark).await);
        for stmt in [
            "DELETE claim WHERE cycle > $w",
            "DELETE skill WHERE cycle > $w",
            "DELETE derived WHERE cycle > $w",
        ] {
            if let Err(e) = self.db.query(stmt).bind(("w", watermark)).await {
                tracing::warn!(error = %e, "rollback delete failed");
            }
        }
        dropped
    }

    async fn knn(&self, qvec: &[f32], k: usize) -> Vec<ClaimId> {
        let mut resp = match self
            .db
            .query("SELECT cid, embedding FROM claim WHERE status = 'verified'")
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "knn query failed");
                return Vec::new();
            }
        };
        let rows: Vec<EmbRow> = resp.take(0).unwrap_or_default();
        let mut scored: Vec<(u64, f32)> = rows
            .into_iter()
            .map(|r| (to_u64(r.cid), Self::cosine(qvec, &r.embedding)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        scored
            .into_iter()
            .take(k)
            .map(|(cid, _)| ClaimId(cid))
            .collect()
    }

    async fn beam(&self, seeds: &[ConceptId], b: usize, depth: usize) -> Vec<Path> {
        // Seed frontier: claims `about` the seed concepts.
        let seed_ids: Vec<i64> = seeds.iter().map(|c| to_i64(c.0)).collect();
        let mut frontier: Vec<Path> = Vec::new();
        let about = match self
            .db
            .query("SELECT about FROM concept WHERE cid IN $s")
            .bind(("s", seed_ids))
            .await
        {
            Ok(mut resp) => resp.take::<Vec<AboutRow>>(0).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "beam seed query failed");
                Vec::new()
            }
        };
        for row in about {
            for cid in row.about {
                frontier.push(Path::new(vec![ClaimId(to_u64(cid))], 1.0));
            }
        }
        let mut results: Vec<Path> = frontier.clone();
        for d in 0..depth {
            let mut next: Vec<Path> = Vec::new();
            for path in &frontier {
                let Some(tail) = path.claims.last() else {
                    continue;
                };
                let parents = match self
                    .db
                    .query("SELECT parent FROM derived WHERE child = $c")
                    .bind(("c", to_i64(tail.0)))
                    .await
                {
                    Ok(mut resp) => resp.take::<Vec<ParentRow>>(0).unwrap_or_default(),
                    Err(e) => {
                        tracing::warn!(error = %e, "beam expand query failed");
                        Vec::new()
                    }
                };
                for p in parents {
                    let mut claims = path.claims.clone();
                    claims.push(ClaimId(to_u64(p.parent)));
                    // Deeper premises rank lower, mirroring the in-memory
                    // confidence-product ordering.
                    let score = 1.0 / (2.0 + f64::from(u32::try_from(d).unwrap_or(0)));
                    next.push(Path::new(claims, score));
                }
            }
            next.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            next.truncate(b.max(1));
            if next.is_empty() {
                break;
            }
            results.extend(next.iter().cloned());
            frontier = next;
        }
        results
    }

    async fn synergy_count(&self) -> usize {
        self.count_where(
            "claim WHERE status = 'verified' AND array::len(families) >= 2",
            0,
        )
        .await
    }

    async fn verified_claim_count(&self) -> usize {
        self.count_where("claim WHERE status = 'verified'", 0).await
    }
}

/// Register a concept node and the claims it is `about` (seed helper for the
/// graph path). Lives here rather than on the trait because seeding concepts
/// is a host concern, not part of the cycle loop.
pub async fn add_concept(
    store: &SurrealKnowledgeStore,
    concept: ConceptId,
    about: &[ClaimId],
) -> Result<(), StoreError> {
    let about_ids: Vec<i64> = about.iter().map(|c| to_i64(c.0)).collect();
    store
        .db
        .query("CREATE concept CONTENT { cid: $cid, about: $about, confidence: 1.0 }")
        .bind(("cid", to_i64(concept.0)))
        .bind(("about", about_ids))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_rsi::kg::{Claim, ClaimStatus, ModelId, Provenance};

    fn claim(id: u64, cycle: u32, families: &[&str], emb: &[f32]) -> Claim {
        let provenance = Provenance::new(
            vec![ModelId("m".into())],
            families.iter().map(|&s| s.to_owned()).collect(),
            Vec::new(),
            1,
            cycle,
        );
        Claim::new(
            ClaimId(id),
            format!("c{id}"),
            emb.to_vec(),
            0.9,
            ClaimStatus::Verified,
            provenance,
        )
    }

    #[tokio::test]
    async fn admit_and_knn_round_trip_through_surrealdb() {
        let mut store = SurrealKnowledgeStore::open_in_memory().await.unwrap();
        store
            .admit(AdmitBatch::new(
                vec![
                    claim(1, 0, &["openai"], &[1.0, 0.0]),
                    claim(2, 0, &["openai"], &[0.0, 1.0]),
                ],
                Vec::new(),
                Vec::new(),
            ))
            .await;
        assert_eq!(store.verified_claim_count().await, 2);
        let near = store.knn(&[1.0, 0.0], 2).await;
        assert_eq!(near.first(), Some(&ClaimId(1)));
    }

    #[tokio::test]
    async fn synergy_count_requires_two_families() {
        let mut store = SurrealKnowledgeStore::open_in_memory().await.unwrap();
        store
            .admit(AdmitBatch::new(
                vec![
                    claim(1, 0, &["openai"], &[1.0]),
                    claim(2, 0, &["openai", "anthropic"], &[1.0]),
                ],
                Vec::new(),
                Vec::new(),
            ))
            .await;
        assert_eq!(store.synergy_count().await, 1);
    }

    #[tokio::test]
    async fn rollback_drops_items_after_the_checkpoint() {
        let mut store = SurrealKnowledgeStore::open_in_memory().await.unwrap();
        store
            .admit(AdmitBatch::new(
                vec![claim(1, 1, &["openai"], &[1.0])],
                Vec::new(),
                Vec::new(),
            ))
            .await;
        let snap = store.checkpoint(1, "cycle-1").await;
        store
            .admit(AdmitBatch::new(
                vec![claim(2, 2, &["openai"], &[1.0])],
                Vec::new(),
                Vec::new(),
            ))
            .await;
        assert_eq!(store.verified_claim_count().await, 2);
        let dropped = store.rollback(snap).await;
        assert_eq!(dropped, 1);
        assert_eq!(store.verified_claim_count().await, 1);
    }

    #[tokio::test]
    async fn beam_follows_derived_edges_from_a_seed_concept() {
        let mut store = SurrealKnowledgeStore::open_in_memory().await.unwrap();
        store
            .admit(AdmitBatch::new(
                vec![
                    claim(1, 0, &["openai"], &[1.0]),
                    claim(2, 0, &["anthropic"], &[1.0]),
                ],
                Vec::new(),
                vec![(ClaimId(1), ClaimId(2))],
            ))
            .await;
        add_concept(&store, ConceptId(10), &[ClaimId(1)])
            .await
            .unwrap();
        let paths = store.beam(&[ConceptId(10)], 4, 2).await;
        assert!(paths.iter().any(|p| p.claims.contains(&ClaimId(2))));
    }
}
