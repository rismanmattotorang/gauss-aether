//! Async live-backend surface (paper §V.B Tokio concurrency model).
//!
//! The sync [`crate::engine::RsiEngine`] is the deterministic, replayable
//! core. Live deployments, however, talk to **async** backends: frontier
//! experts behind an HTTP gateway and a SurrealDB knowledge store. This module
//! mirrors the engine over async traits so a host can drive Algorithm 1
//! against real I/O while reusing every pure component (the LinUCB router, the
//! DualRAG fusion, the tiered verifier, the SAHOO drift gate, the convergence
//! detector).
//!
//! It is feature-gated (`async`, on by default) and still ships no concrete
//! I/O — the trait surface is the contract the `gaussclaw-rsi` crate
//! implements against live SurrealDB + provider backends.

use async_trait::async_trait;

use crate::dualrag::{DualRagParams, PackedContext};
use crate::engine::{
    saturating_u32, CycleInput, CycleReport, EngineConfig, ExpertOutput, Lcg, Query,
};
use crate::event::CycleEvent;
use crate::fusion::{pack_premises_first, reciprocal_rank_fusion, RankedList};
use crate::gdi::{DriftGate, DriftVerdict};
use crate::kg::{AdmitBatch, ConceptId, ModelId, Path, SnapshotId};
use crate::router::LinUcbRouter;
use crate::state::ClaimId;
use crate::verify::{certify_skill, verify_claim, Verdict, VerifierConfig};

/// Async counterpart of [`crate::engine::Expert`]: a frozen frontier model
/// reached over the network.
#[async_trait]
pub trait AsyncExpert: Send + Sync {
    /// The expert's model identifier.
    fn id(&self) -> ModelId;
    /// The expert's provider family (used by the Tier-2 cross-family quorum).
    fn family(&self) -> String;
    /// Generate candidates for a query given the retrieved context.
    async fn generate(&self, query: &Query, context: &PackedContext) -> ExpertOutput;
}

/// Async counterpart of [`crate::kg::KnowledgeStore`]: the live, transactional
/// KnowledgeGraph state (SurrealDB in production).
#[async_trait]
pub trait AsyncKnowledgeStore: Send + Sync {
    /// Admit a cycle's batch into the verified state.
    async fn admit(&mut self, batch: AdmitBatch);
    /// Take a named snapshot at the current cycle; returns its handle.
    async fn checkpoint(&mut self, cycle: u32, label: &str) -> SnapshotId;
    /// Roll back to a snapshot: discard everything admitted after it.
    async fn rollback(&mut self, to: SnapshotId) -> usize;
    /// Vector path: top-`k` verified claims by similarity to `qvec`.
    async fn knn(&self, qvec: &[f32], k: usize) -> Vec<ClaimId>;
    /// Graph path: typed beam search of width `b`, depth `depth`.
    async fn beam(&self, seeds: &[ConceptId], b: usize, depth: usize) -> Vec<Path>;
    /// Synergy count (Theorem 2(b)): verified claims spanning `>= 2` families.
    async fn synergy_count(&self) -> usize;
    /// Count of verified claims (the verified `|K|`).
    async fn verified_claim_count(&self) -> usize;
}

/// Async DualRAG retrieval (Algorithm 2) over an [`AsyncKnowledgeStore`]:
/// awaits the vector and graph paths, then reuses the pure fusion stage.
pub async fn retrieve_async<S: AsyncKnowledgeStore + ?Sized>(
    store: &S,
    qvec: &[f32],
    seeds: &[ConceptId],
    params: &DualRagParams,
) -> PackedContext {
    let vector = store.knn(qvec, params.k_vector).await;
    let mut graph: Vec<ClaimId> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in store.beam(seeds, params.beam, params.depth).await {
        for c in path.claims {
            if seen.insert(c) {
                graph.push(c);
            }
        }
    }
    let fused = reciprocal_rank_fusion(
        &[RankedList::new(&vector), RankedList::new(&graph)],
        params.rrf_k,
    );
    let fused_ids: Vec<ClaimId> = fused.into_iter().map(|(id, _)| id).collect();
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

/// Async RSI Loop Engine: iterates the operator Φ (Algorithm 1) against live
/// async backends, reusing every pure component of the sync engine.
pub struct AsyncRsiEngine<S: AsyncKnowledgeStore> {
    store: S,
    router: LinUcbRouter,
    experts: Vec<Box<dyn AsyncExpert>>,
    cfg: EngineConfig,
    drift_gate: DriftGate,
    dualrag: DualRagParams,
    verifier: VerifierConfig,
    cycle: u32,
    last_checkpoint: SnapshotId,
    low_admit_streak: u32,
    min_tier: u8,
    events: Vec<CycleEvent>,
    rng: Lcg,
}

impl<S: AsyncKnowledgeStore> AsyncRsiEngine<S> {
    /// Construct an engine. The router's arm count must equal the number of
    /// experts.
    ///
    /// # Panics
    /// Panics if `router.arms() != experts.len()`.
    #[must_use]
    pub fn new(
        store: S,
        router: LinUcbRouter,
        experts: Vec<Box<dyn AsyncExpert>>,
        cfg: EngineConfig,
        drift_gate: DriftGate,
        dualrag: DualRagParams,
        verifier: VerifierConfig,
    ) -> Self {
        assert_eq!(
            router.arms(),
            experts.len(),
            "router arm count must match expert count"
        );
        Self {
            store,
            router,
            experts,
            cfg,
            drift_gate,
            dualrag,
            verifier,
            cycle: 0,
            last_checkpoint: SnapshotId(0),
            low_admit_streak: 0,
            min_tier: 3,
            events: Vec::new(),
            rng: Lcg::new(0x5DEE_CE66_D8B6_9C15),
        }
    }

    /// Borrow the underlying store.
    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// The events emitted so far.
    #[must_use]
    pub fn events(&self) -> &[CycleEvent] {
        &self.events
    }

    /// The current cycle index.
    #[must_use]
    pub const fn cycle(&self) -> u32 {
        self.cycle
    }

    /// Run one cycle of Φ (Algorithm 1) against the live backends.
    pub async fn run_cycle(&mut self, input: &CycleInput) -> CycleReport {
        let t = self.cycle;
        self.events.push(CycleEvent::Started { t });

        let mut batch = AdmitBatch::default();
        let arms = self.experts.len();
        let mut emitted = vec![0u32; arms];
        let mut admitted = vec![0u32; arms];

        for q in &input.queries {
            self.process_query(q, t, &mut emitted, &mut admitted, &mut batch)
                .await;
        }

        let admitted_mass = saturating_u32(batch.claims.len().saturating_add(batch.skills.len()));
        let admitted_mass_f = f64::from(admitted_mass);
        let gdi = input.drift.gdi(&self.drift_gate.weights);

        if self.drift_gate.evaluate(&input.drift, input.critical_ok) == DriftVerdict::Rollback {
            let _dropped = self.store.rollback(self.last_checkpoint).await;
            self.min_tier = self.min_tier.saturating_sub(1).max(1);
            self.events.push(CycleEvent::Drift {
                drift: input.drift,
                gdi,
            });
            self.events.push(CycleEvent::RolledBack {
                to: self.last_checkpoint.0,
            });
            self.cycle = self.cycle.saturating_add(1);
            return CycleReport {
                cycle: t,
                admitted_mass: 0.0,
                gdi,
                rolled_back: true,
                converged: false,
                synergy_count: self.store.synergy_count().await,
            };
        }

        self.store.admit(batch).await;
        self.last_checkpoint = self.store.checkpoint(t, &format!("cycle-{t}")).await;
        self.events.push(CycleEvent::Admitted {
            mass: admitted_mass_f,
        });
        self.events.push(CycleEvent::Drift {
            drift: input.drift,
            gdi,
        });
        self.update_rewards(&input.queries, &emitted, &admitted);

        if admitted_mass_f < self.cfg.eps {
            self.low_admit_streak = self.low_admit_streak.saturating_add(1);
        } else {
            self.low_admit_streak = 0;
        }
        let converged = self.low_admit_streak >= self.cfg.patience;
        if converged {
            self.events.push(CycleEvent::Converged { t });
        }
        self.cycle = self.cycle.saturating_add(1);
        CycleReport {
            cycle: t,
            admitted_mass: admitted_mass_f,
            gdi,
            rolled_back: false,
            converged,
            synergy_count: self.store.synergy_count().await,
        }
    }

    /// Run cycles until convergence, the budget `Bmax`, or a safety halt,
    /// pulling each cycle's input from `next` (Proposition 3 termination).
    pub async fn run<F>(&mut self, mut next: F) -> Vec<CycleReport>
    where
        F: FnMut(u32) -> CycleInput,
    {
        let mut reports = Vec::new();
        while self.cycle < self.cfg.budget {
            let input = next(self.cycle);
            let report = self.run_cycle(&input).await;
            let converged = report.converged;
            reports.push(report);
            if converged {
                break;
            }
        }
        reports
    }

    async fn process_query(
        &mut self,
        q: &Query,
        t: u32,
        emitted: &mut [u32],
        admitted: &mut [u32],
        batch: &mut AdmitBatch,
    ) {
        let draw = self.rng.next_f64();
        let explore_arm = self.rng.next_index(self.experts.len());
        let dispatch = self.router.route(
            &q.context_features,
            self.cfg.fanout,
            self.cfg.gamma,
            draw,
            explore_arm,
        );
        let ctx = retrieve_async(&self.store, &q.embedding, &q.seeds, &self.dualrag).await;
        for aw in &dispatch.arms {
            let Some(expert) = self.experts.get(aw.arm) else {
                continue;
            };
            let out = expert.generate(q, &ctx).await;
            if let Some(e) = emitted.get_mut(aw.arm) {
                *e = e.saturating_add(saturating_u32(
                    out.claims.len().saturating_add(out.skills.len()),
                ));
            }
            for cand in out.claims {
                if let Verdict::Pass { tier } = verify_claim(&cand.signals, &self.verifier) {
                    if tier <= self.min_tier {
                        let mut claim = cand.claim;
                        claim.provenance.cycle = t;
                        let child = claim.id;
                        batch.claims.push(claim);
                        for parent in cand.premises {
                            batch.derived_from.push((child, parent));
                        }
                        if let Some(a) = admitted.get_mut(aw.arm) {
                            *a = a.saturating_add(1);
                        }
                    }
                }
            }
            for cand in out.skills {
                if certify_skill(cand.p_hat, cand.m, cand.delta, self.cfg.tau_skill).is_some() {
                    let mut skill = cand.skill;
                    skill.cycle = t;
                    batch.skills.push(skill);
                    if let Some(a) = admitted.get_mut(aw.arm) {
                        *a = a.saturating_add(1);
                    }
                }
            }
        }
    }

    fn update_rewards(&mut self, queries: &[Query], emitted: &[u32], admitted: &[u32]) {
        let Some(dim) = queries.first().map(|q| q.context_features.len()) else {
            return;
        };
        let mut avg = vec![0.0_f64; dim];
        for q in queries {
            for (a, x) in avg.iter_mut().zip(q.context_features.iter()) {
                *a += *x;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let n = queries.len() as f64;
        if n > 0.0 {
            for a in &mut avg {
                *a /= n;
            }
        }
        for arm in 0..self.experts.len() {
            let em = emitted.get(arm).copied().unwrap_or(0);
            if em == 0 {
                continue;
            }
            let ad = admitted.get(arm).copied().unwrap_or(0);
            let utility = f64::from(ad) / f64::from(em);
            let reward = crate::router::cost_adjusted_reward(
                utility,
                0.0,
                0.0,
                self.cfg.lambda_dollar,
                self.cfg.lambda_latency,
            );
            self.router.update(arm, &avg, reward);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{CandidateClaim, CycleInput};
    use crate::gdi::{DriftComponents, DriftWeights};
    use crate::kg::{
        Claim, ClaimStatus, Concept, InMemoryKnowledgeStore, KnowledgeStore, Provenance,
    };
    use crate::verify::ClaimCandidate;

    /// In-memory store wrapped to satisfy the async trait (mirrors the live
    /// SurrealDB store's interface for tests without a database).
    struct AsyncMemStore(InMemoryKnowledgeStore);

    #[async_trait]
    impl AsyncKnowledgeStore for AsyncMemStore {
        async fn admit(&mut self, batch: AdmitBatch) {
            self.0.admit(batch);
        }
        async fn checkpoint(&mut self, cycle: u32, label: &str) -> SnapshotId {
            self.0.checkpoint(cycle, label)
        }
        async fn rollback(&mut self, to: SnapshotId) -> usize {
            self.0.rollback(to)
        }
        async fn knn(&self, qvec: &[f32], k: usize) -> Vec<ClaimId> {
            self.0.knn(qvec, k)
        }
        async fn beam(&self, seeds: &[ConceptId], b: usize, depth: usize) -> Vec<Path> {
            self.0.beam(seeds, b, depth)
        }
        async fn synergy_count(&self) -> usize {
            self.0.synergy_count()
        }
        async fn verified_claim_count(&self) -> usize {
            self.0.verified_claim_count()
        }
    }

    struct StubAsyncExpert {
        family: String,
        next: std::sync::atomic::AtomicU64,
    }

    #[async_trait]
    impl AsyncExpert for StubAsyncExpert {
        fn id(&self) -> ModelId {
            ModelId(format!("stub/{}", self.family))
        }
        fn family(&self) -> String {
            self.family.clone()
        }
        async fn generate(&self, _q: &Query, _ctx: &PackedContext) -> ExpertOutput {
            let id = self
                .next
                .fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
            let claim = Claim {
                id: ClaimId(id),
                content: format!("c{id}"),
                embedding: vec![1.0, 0.0],
                confidence: 0.95,
                status: ClaimStatus::Verified,
                provenance: Provenance {
                    models: vec![ModelId(format!("stub/{}", self.family))],
                    model_families: ["openai".to_owned(), "anthropic".to_owned()]
                        .into_iter()
                        .collect(),
                    premises: Vec::new(),
                    verifier_tier: 1,
                    cycle: 0,
                },
            };
            ExpertOutput {
                answer_key: 1,
                claims: vec![CandidateClaim {
                    claim,
                    signals: ClaimCandidate {
                        tier1_checkable: true,
                        tier1_passes: true,
                        cites_sources: false,
                        votes: Vec::new(),
                        tier3_judge_approves: false,
                        touches_probe: false,
                    },
                    premises: Vec::new(),
                }],
                skills: Vec::new(),
            }
        }
    }

    fn engine() -> AsyncRsiEngine<AsyncMemStore> {
        let experts: Vec<Box<dyn AsyncExpert>> = vec![
            Box::new(StubAsyncExpert {
                family: "openai".into(),
                next: std::sync::atomic::AtomicU64::new(1),
            }),
            Box::new(StubAsyncExpert {
                family: "anthropic".into(),
                next: std::sync::atomic::AtomicU64::new(2),
            }),
        ];
        AsyncRsiEngine::new(
            AsyncMemStore(InMemoryKnowledgeStore::new()),
            LinUcbRouter::new(2, 2, 0.6, 0.05),
            experts,
            EngineConfig::default(),
            DriftGate::new(DriftWeights::default(), 0.5),
            DualRagParams::default(),
            VerifierConfig::default(),
        )
    }

    fn query() -> Query {
        Query {
            id: 1,
            embedding: vec![1.0, 0.0],
            seeds: Vec::new(),
            context_features: vec![1.0, 0.0],
        }
    }

    #[tokio::test]
    async fn async_cycle_accrues_state() {
        let mut e = engine();
        let report = e
            .run_cycle(&CycleInput {
                queries: vec![query()],
                drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
                critical_ok: true,
            })
            .await;
        assert!(report.admitted_mass > 0.0);
        assert!(e.store().verified_claim_count().await > 0);
    }

    #[tokio::test]
    async fn async_high_drift_rolls_back() {
        let mut e = engine();
        e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        })
        .await;
        let report = e
            .run_cycle(&CycleInput {
                queries: vec![query()],
                drift: DriftComponents::new(0.9, 0.9, 0.9, 0.9),
                critical_ok: true,
            })
            .await;
        assert!(report.rolled_back);
    }

    #[tokio::test]
    async fn async_retrieve_surfaces_graph_premises() {
        let mut store = InMemoryKnowledgeStore::new();
        store.admit(AdmitBatch {
            claims: vec![Claim {
                id: ClaimId(1),
                content: "c1".into(),
                embedding: vec![1.0, 0.0],
                confidence: 0.9,
                status: ClaimStatus::Verified,
                provenance: Provenance::default(),
            }],
            skills: Vec::new(),
            derived_from: Vec::new(),
        });
        store.add_concept(
            Concept {
                id: ConceptId(10),
                name: "seed".into(),
                embedding: vec![1.0, 0.0],
            },
            vec![ClaimId(1)],
        );
        let async_store = AsyncMemStore(store);
        let ctx = retrieve_async(
            &async_store,
            &[1.0, 0.0],
            &[ConceptId(10)],
            &DualRagParams::default(),
        )
        .await;
        assert!(ctx.items.contains(&ClaimId(1)));
        assert!(ctx.premise_count >= 1);
    }
}
