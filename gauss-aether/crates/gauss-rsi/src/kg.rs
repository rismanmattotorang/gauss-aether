//! KnowledgeGraph — the materialized RSI state `x = (K, S)` (paper §IV.C,
//! Appendix A).
//!
//! The paper stores the state in a single multi-model SurrealDB instance:
//! `claim` / `skill` / `concept` / `model` / `task` / `snapshot` node tables,
//! typed `RELATION` edge tables (`about`, `relates`, `supports`,
//! `contradicts`, `derived_from`, `evidences`, `requires`, `certified_on`,
//! `involves`, `emitted_by`), and HNSW vector indexes on the embedding
//! fields. The canonical SurrealQL is reproduced verbatim in
//! [`SCHEMA_SURREALQL`] so the live `gauss-memory` backend (Phase 2 of
//! `AGENT0_INTEGRATION.md`) can apply it unchanged.
//!
//! This module ships the **typed Rust models** plus the [`KnowledgeStore`]
//! trait and a deterministic in-memory reference implementation
//! ([`InMemoryKnowledgeStore`]) — the same "trait + in-memory reference first,
//! live backend behind a feature" pattern used by `gauss-curator` and
//! `gauss-memory`. Every admitted claim carries [`Provenance`] (emitting
//! models, premise edges, verifier tier, cycle index), which is what makes the
//! synergy estimate `µ̂(Σ ∩ K_T)` of Theorem 2 a directly queryable count
//! ([`KnowledgeStore::synergy_count`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::state::{ClaimId, SkillId};

/// The complete Gauss-Agent0 SurrealDB schema (paper Appendix A, Listing 5),
/// verbatim. Applied unchanged by the live `gauss-memory` backend.
pub const SCHEMA_SURREALQL: &str = include_str!("schema.surql");

/// An OpenRouter model slug identifier (e.g. `anthropic/claude-sonnet-4.5`).
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

/// A concept node identifier (a named entity the graph path expands from).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConceptId(pub u64);

/// A named, immutable per-cycle snapshot handle (paper §IV.C). Equal to the
/// cycle index at which the checkpoint was taken, so rollback is a watermark
/// reset (paper §V.D).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotId(pub u32);

/// Lifecycle status of a claim (paper Listing 1: `candidate|verified|falsified`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClaimStatus {
    /// Emitted but not yet certified.
    Candidate,
    /// Certified by the VerifierAgent.
    Verified,
    /// Rejected by the VerifierAgent (or quarantined by a cascade).
    Falsified,
}

/// Provenance of an admitted item (paper §IV.C). Stored on every claim so the
/// synergy estimate of Theorem 2 is queryable and every item has an auditable
/// derivation trail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Provenance {
    /// The model(s) that emitted the item.
    pub models: Vec<ModelId>,
    /// Distinct model *families* across the emitting models and premises — the
    /// `>= 2` test of the synergy count (Listing 5, Theorem 2(b)).
    pub model_families: BTreeSet<String>,
    /// Premise claims this item was derived from (the `derived_from` edges).
    pub premises: Vec<ClaimId>,
    /// VerifierAgent tier that admitted the item (1 strongest .. 3 weakest).
    pub verifier_tier: u8,
    /// Admission cycle index `t`.
    pub cycle: u32,
}

impl Provenance {
    /// Construct provenance for an admitted item.
    #[must_use]
    pub fn new(
        models: Vec<ModelId>,
        model_families: BTreeSet<String>,
        premises: Vec<ClaimId>,
        verifier_tier: u8,
        cycle: u32,
    ) -> Self {
        Self {
            models,
            model_families,
            premises,
            verifier_tier,
            cycle,
        }
    }

    /// Whether this item's derivation spans at least two model families — the
    /// per-item synergy predicate of Theorem 2(b).
    #[must_use]
    pub fn is_synergistic(&self) -> bool {
        self.model_families.len() >= 2
    }
}

/// A verifiable knowledge item (paper Listing 5, `claim` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Claim {
    /// Stable identifier.
    pub id: ClaimId,
    /// Natural-language content.
    pub content: String,
    /// Embedding for the HNSW vector path.
    pub embedding: Vec<f32>,
    /// Confidence in `[0, 1]`.
    pub confidence: f64,
    /// Lifecycle status.
    pub status: ClaimStatus,
    /// Provenance + cycle index.
    pub provenance: Provenance,
}

impl Claim {
    /// Construct a claim.
    #[must_use]
    pub fn new(
        id: ClaimId,
        content: String,
        embedding: Vec<f32>,
        confidence: f64,
        status: ClaimStatus,
        provenance: Provenance,
    ) -> Self {
        Self {
            id,
            content,
            embedding,
            confidence,
            status,
            provenance,
        }
    }
}

/// A certifiable skill (paper Listing 5, `skill` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Skill {
    /// Stable identifier.
    pub id: SkillId,
    /// Skill name.
    pub name: String,
    /// Type/signature string.
    pub signature: String,
    /// Source code (executed sandboxed by the Tier-1 verifier).
    pub code: String,
    /// Source language.
    pub lang: String,
    /// Embedding for the vector path.
    pub embedding: Vec<f32>,
    /// Empirical pass rate `p̂` (paper Eq. 11).
    pub pass_rate: f64,
    /// PAC lower confidence bound `ci_low` (Eq. 11).
    pub ci_low: f64,
    /// Number of evaluation tasks `m` (Eq. 11).
    pub m_tests: u32,
    /// Admission cycle index.
    pub cycle: u32,
}

impl Skill {
    /// Construct a skill.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SkillId,
        name: String,
        signature: String,
        code: String,
        lang: String,
        embedding: Vec<f32>,
        pass_rate: f64,
        ci_low: f64,
        m_tests: u32,
        cycle: u32,
    ) -> Self {
        Self {
            id,
            name,
            signature,
            code,
            lang,
            embedding,
            pass_rate,
            ci_low,
            m_tests,
            cycle,
        }
    }
}

/// A concept node (paper Listing 5, `concept` table): the seed of the graph
/// path's beam search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Concept {
    /// Stable identifier.
    pub id: ConceptId,
    /// Concept name (unique).
    pub name: String,
    /// Embedding.
    pub embedding: Vec<f32>,
}

/// A frontier model record with cost metadata feeding Eq. (4) (paper
/// Listing 5, `model` table; Appendix C pool).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelRec {
    /// OpenRouter slug.
    pub slug: ModelId,
    /// Provider family (used by the cross-family quorum, paper §IV.G).
    pub family: String,
    /// Input price (USD / 1e6 tokens).
    pub price_in: f64,
    /// Output price (USD / 1e6 tokens).
    pub price_out: f64,
    /// Rolling p50 latency, ms.
    pub latency: f64,
}

/// One graph-path traversal result: an ordered premise chain and its score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Path {
    /// The claims on the path, seed-adjacent first.
    pub claims: Vec<ClaimId>,
    /// Path score `∏ conf(e) · rel(π, φ)` (Algorithm 2).
    pub score: f64,
}

impl Path {
    /// Construct a graph-path result.
    #[must_use]
    pub fn new(claims: Vec<ClaimId>, score: f64) -> Self {
        Self { claims, score }
    }
}

/// The admitted batch of one cycle (the `Aₜ` of Eq. 2), with full items so the
/// store can record provenance and edges.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AdmitBatch {
    /// Admitted claims.
    pub claims: Vec<Claim>,
    /// Admitted skills.
    pub skills: Vec<Skill>,
    /// `derived_from` edges `(child, parent)` recorded for the synergy trail.
    pub derived_from: Vec<(ClaimId, ClaimId)>,
}

impl AdmitBatch {
    /// Construct an admit batch.
    #[must_use]
    pub fn new(
        claims: Vec<Claim>,
        skills: Vec<Skill>,
        derived_from: Vec<(ClaimId, ClaimId)>,
    ) -> Self {
        Self {
            claims,
            skills,
            derived_from,
        }
    }
}

/// The KnowledgeGraph state interface (paper §IV.C). Supplies the finite,
/// auditable state of Assumption 2; named per-cycle snapshots make rollback
/// the O(1) operation the safety gate assumes.
pub trait KnowledgeStore {
    /// Admit a cycle's batch into the verified state.
    fn admit(&mut self, batch: AdmitBatch);

    /// Take a named snapshot at the current cycle; returns its handle.
    fn checkpoint(&mut self, cycle: u32, label: &str) -> SnapshotId;

    /// Roll back to a snapshot: discard everything admitted after it (paper
    /// Listing 5 rollback query). Returns the number of items dropped.
    fn rollback(&mut self, to: SnapshotId) -> usize;

    /// Vector path (Algorithm 2): top-`k` verified claims by cosine similarity
    /// to `qvec`, best-first.
    fn knn(&self, qvec: &[f32], k: usize) -> Vec<ClaimId>;

    /// Graph path (Algorithm 2): typed beam search of width `b` and depth
    /// `depth` over `derived_from`/`supports` edges from the seed concepts'
    /// adjacent claims, best-first.
    fn beam(&self, seeds: &[ConceptId], b: usize, depth: usize) -> Vec<Path>;

    /// Synergy count for Theorem 2(b): verified claims whose provenance spans
    /// `>= 2` model families — a direct estimate of `µ(Σ ∩ K_T)`.
    fn synergy_count(&self) -> usize;

    /// Quarantine the derivation cascade rooted at a falsified claim
    /// (Proposition 2): every claim transitively `derived_from` it reverts to
    /// `Falsified`. Returns the quarantined claim ids.
    fn quarantine_cascade(&mut self, from: ClaimId) -> Vec<ClaimId>;
}

/// Deterministic in-memory reference [`KnowledgeStore`].
///
/// Mirrors the SurrealDB semantics closely enough for the conformance suite
/// and the engine's integration tests; the live backend lands in
/// `gauss-memory`.
#[derive(Debug, Clone, Default)]
pub struct InMemoryKnowledgeStore {
    claims: BTreeMap<ClaimId, Claim>,
    skills: BTreeMap<SkillId, Skill>,
    concepts: BTreeMap<ConceptId, Concept>,
    /// concept -> claims it is `about`.
    about: BTreeMap<ConceptId, Vec<ClaimId>>,
    /// child -> parents (the `derived_from` adjacency).
    derived_from: BTreeMap<ClaimId, Vec<ClaimId>>,
    /// parent -> children (reverse adjacency, for cascade quarantine).
    derives: BTreeMap<ClaimId, Vec<ClaimId>>,
    snapshots: BTreeMap<SnapshotId, String>,
}

impl InMemoryKnowledgeStore {
    /// Construct an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a concept node and its `about` claims (test/seed helper).
    pub fn add_concept(&mut self, concept: Concept, about: Vec<ClaimId>) {
        self.about.insert(concept.id, about);
        self.concepts.insert(concept.id, concept);
    }

    /// Read a claim by id.
    #[must_use]
    pub fn claim(&self, id: ClaimId) -> Option<&Claim> {
        self.claims.get(&id)
    }

    /// Read a skill by id.
    #[must_use]
    pub fn skill(&self, id: SkillId) -> Option<&Skill> {
        self.skills.get(&id)
    }

    /// Count of verified claims (the verified `|K|`).
    #[must_use]
    pub fn verified_claim_count(&self) -> usize {
        self.claims
            .values()
            .filter(|c| c.status == ClaimStatus::Verified)
            .count()
    }

    /// Provenance of a claim.
    #[must_use]
    pub fn provenance(&self, id: ClaimId) -> Option<&Provenance> {
        self.claims.get(&id).map(|c| &c.provenance)
    }
}

impl KnowledgeStore for InMemoryKnowledgeStore {
    fn admit(&mut self, batch: AdmitBatch) {
        for claim in batch.claims {
            self.claims.insert(claim.id, claim);
        }
        for skill in batch.skills {
            self.skills.insert(skill.id, skill);
        }
        for (child, parent) in batch.derived_from {
            self.derived_from.entry(child).or_default().push(parent);
            self.derives.entry(parent).or_default().push(child);
        }
    }

    fn checkpoint(&mut self, cycle: u32, label: &str) -> SnapshotId {
        let id = SnapshotId(cycle);
        self.snapshots.insert(id, label.to_owned());
        id
    }

    fn rollback(&mut self, to: SnapshotId) -> usize {
        let watermark = to.0;
        let drop_claims: Vec<ClaimId> = self
            .claims
            .iter()
            .filter(|(_, c)| c.provenance.cycle > watermark)
            .map(|(id, _)| *id)
            .collect();
        let drop_skills: Vec<SkillId> = self
            .skills
            .iter()
            .filter(|(_, s)| s.cycle > watermark)
            .map(|(id, _)| *id)
            .collect();
        let dropped = drop_claims.len().saturating_add(drop_skills.len());
        for id in &drop_claims {
            self.claims.remove(id);
            self.derived_from.remove(id);
            self.derives.remove(id);
        }
        for id in &drop_skills {
            self.skills.remove(id);
        }
        // Drop dangling edges to removed claims.
        let removed: BTreeSet<ClaimId> = drop_claims.iter().copied().collect();
        for parents in self.derived_from.values_mut() {
            parents.retain(|p| !removed.contains(p));
        }
        for children in self.derives.values_mut() {
            children.retain(|c| !removed.contains(c));
        }
        dropped
    }

    fn knn(&self, qvec: &[f32], k: usize) -> Vec<ClaimId> {
        let mut scored: Vec<(ClaimId, f32)> = self
            .claims
            .values()
            .filter(|c| c.status == ClaimStatus::Verified)
            .map(|c| (c.id, cosine(qvec, &c.embedding)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        scored.into_iter().take(k).map(|(id, _)| id).collect()
    }

    fn beam(&self, seeds: &[ConceptId], b: usize, depth: usize) -> Vec<Path> {
        // Seed frontier: claims `about` the seed concepts.
        let mut frontier: Vec<Path> = Vec::new();
        for seed in seeds {
            if let Some(claims) = self.about.get(seed) {
                for &c in claims {
                    if self
                        .claims
                        .get(&c)
                        .is_some_and(|cl| cl.status == ClaimStatus::Verified)
                    {
                        frontier.push(Path {
                            claims: vec![c],
                            score: self.claims.get(&c).map_or(0.0, |cl| cl.confidence),
                        });
                    }
                }
            }
        }
        let mut results: Vec<Path> = frontier.clone();
        for _ in 0..depth {
            let mut next: Vec<Path> = Vec::new();
            for path in &frontier {
                if let Some(&tail) = path.claims.last() {
                    if let Some(parents) = self.derived_from.get(&tail) {
                        for &parent in parents {
                            if let Some(p) = self.claims.get(&parent) {
                                if p.status != ClaimStatus::Verified {
                                    continue;
                                }
                                let mut claims = path.claims.clone();
                                claims.push(parent);
                                next.push(Path {
                                    claims,
                                    score: path.score * p.confidence,
                                });
                            }
                        }
                    }
                }
            }
            // Keep top-`b` paths by score (the beam).
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
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    fn synergy_count(&self) -> usize {
        self.claims
            .values()
            .filter(|c| c.status == ClaimStatus::Verified && c.provenance.is_synergistic())
            .count()
    }

    fn quarantine_cascade(&mut self, from: ClaimId) -> Vec<ClaimId> {
        let mut quarantined: Vec<ClaimId> = Vec::new();
        let mut queue: VecDeque<ClaimId> = VecDeque::new();
        queue.push_back(from);
        let mut seen: BTreeSet<ClaimId> = BTreeSet::new();
        seen.insert(from);
        while let Some(id) = queue.pop_front() {
            if let Some(c) = self.claims.get_mut(&id) {
                c.status = ClaimStatus::Falsified;
                quarantined.push(id);
            }
            if let Some(children) = self.derives.get(&id) {
                for &child in children {
                    if seen.insert(child) {
                        queue.push_back(child);
                    }
                }
            }
        }
        quarantined
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn verified_claim(id: u64, cycle: u32, families: &[&str], embedding: &[f32]) -> Claim {
        Claim {
            id: ClaimId(id),
            content: format!("claim {id}"),
            embedding: embedding.to_vec(),
            confidence: 0.9,
            status: ClaimStatus::Verified,
            provenance: Provenance {
                models: vec![ModelId("m".into())],
                model_families: families.iter().map(|&s| s.to_owned()).collect(),
                premises: Vec::new(),
                verifier_tier: 1,
                cycle,
            },
        }
    }

    #[test]
    fn schema_contains_the_core_tables() {
        assert!(SCHEMA_SURREALQL.contains("DEFINE TABLE claim"));
        assert!(SCHEMA_SURREALQL.contains("DEFINE TABLE skill"));
        assert!(SCHEMA_SURREALQL.contains("derived_from"));
        assert!(SCHEMA_SURREALQL.contains("HNSW"));
    }

    #[test]
    fn knn_ranks_by_cosine_similarity() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![
                verified_claim(1, 0, &["openai"], &[1.0, 0.0]),
                verified_claim(2, 0, &["openai"], &[0.0, 1.0]),
            ],
            ..AdmitBatch::default()
        });
        let near = store.knn(&[1.0, 0.0], 2);
        assert_eq!(near[0], ClaimId(1));
    }

    #[test]
    fn synergy_count_requires_two_model_families() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![
                verified_claim(1, 0, &["openai"], &[1.0]),
                verified_claim(2, 0, &["openai", "anthropic"], &[1.0]),
            ],
            ..AdmitBatch::default()
        });
        assert_eq!(store.synergy_count(), 1);
    }

    #[test]
    fn rollback_drops_items_admitted_after_the_checkpoint() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![verified_claim(1, 1, &["openai"], &[1.0])],
            ..AdmitBatch::default()
        });
        let snap = store.checkpoint(1, "cycle-1");
        store.admit(AdmitBatch {
            claims: vec![verified_claim(2, 2, &["openai"], &[1.0])],
            ..AdmitBatch::default()
        });
        assert_eq!(store.verified_claim_count(), 2);
        let dropped = store.rollback(snap);
        assert_eq!(dropped, 1);
        assert_eq!(store.verified_claim_count(), 1);
        assert!(store.claim(ClaimId(2)).is_none());
    }

    #[test]
    fn beam_follows_derived_from_edges() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![
                verified_claim(1, 0, &["openai"], &[1.0]),
                verified_claim(2, 0, &["anthropic"], &[1.0]),
            ],
            skills: Vec::new(),
            derived_from: vec![(ClaimId(1), ClaimId(2))],
        });
        store.add_concept(
            Concept {
                id: ConceptId(10),
                name: "seed".into(),
                embedding: vec![1.0],
            },
            vec![ClaimId(1)],
        );
        let paths = store.beam(&[ConceptId(10)], 4, 2);
        // The longest path should reach claim 2 via derived_from from claim 1.
        assert!(paths.iter().any(|p| p.claims.contains(&ClaimId(2))));
    }

    #[test]
    fn cascade_quarantine_falsifies_downstream_claims() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![
                verified_claim(1, 0, &["openai"], &[1.0]),
                verified_claim(2, 0, &["openai"], &[1.0]),
                verified_claim(3, 0, &["openai"], &[1.0]),
            ],
            skills: Vec::new(),
            // 2 derived from 1; 3 derived from 2.
            derived_from: vec![(ClaimId(2), ClaimId(1)), (ClaimId(3), ClaimId(2))],
        });
        // Falsify claim 1 => 2 and 3 must be quarantined too.
        let q = store.quarantine_cascade(ClaimId(1));
        assert!(q.contains(&ClaimId(2)));
        assert!(q.contains(&ClaimId(3)));
        assert_eq!(
            store.claim(ClaimId(3)).unwrap().status,
            ClaimStatus::Falsified
        );
    }
}
