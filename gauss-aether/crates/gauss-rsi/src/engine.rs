//! RSI Loop Engine — the operator Φ iterated to a fixed point (paper §IV.B,
//! Algorithm 1).
//!
//! The engine owns the cycle index `t`, executes one cycle of Φ, maintains
//! per-cycle checkpoints, runs the convergence detector of Theorem 1, and
//! enforces the SAHOO safety gate. One cycle (Algorithm 1) is:
//!
//! ```text
//! curriculum → route → DualRAG → generate → critique → verify
//!            → admit/checkpoint → drift gate → convergence check.
//! ```
//!
//! The loop participants are trait objects ([`Expert`]) and the components of
//! the preceding phases ([`crate::router`], [`crate::dualrag`],
//! [`crate::verify`], [`crate::critic`], [`crate::gdi`],
//! [`crate::kg::KnowledgeStore`]), so the engine is driven deterministically
//! from fixed inputs — the live async Tokio wiring (paper §V.B) is an additive
//! layer over this core. Every admitted batch is realized through the Φ update
//! of [`crate::state`], and the back edge / rollback edge are
//! [`crate::kg::KnowledgeStore`] operations.

use serde::{Deserialize, Serialize};

use crate::critic::self_consistency;
use crate::dualrag::{retrieve, DualRagParams, PackedContext};
use crate::event::CycleEvent;
use crate::gdi::{DriftComponents, DriftGate, DriftVerdict};
use crate::kg::{AdmitBatch, Claim, ConceptId, KnowledgeStore, ModelId, Skill, SnapshotId};
use crate::router::{cost_adjusted_reward, LinUcbRouter};
use crate::state::ClaimId;
use crate::verify::{certify_skill, verify_claim, ClaimCandidate, VerifierConfig};

/// A frontier task routed through one cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Query {
    /// Stable task identifier.
    pub id: u64,
    /// Query embedding for the DualRAG vector path.
    pub embedding: Vec<f32>,
    /// Seed concepts for the DualRAG graph path.
    pub seeds: Vec<ConceptId>,
    /// Router context features `φ(q)` (dimension must match the router).
    pub context_features: Vec<f64>,
}

impl Query {
    /// Construct a query.
    #[must_use]
    pub fn new(
        id: u64,
        embedding: Vec<f32>,
        seeds: Vec<ConceptId>,
        context_features: Vec<f64>,
    ) -> Self {
        Self {
            id,
            embedding,
            seeds,
            context_features,
        }
    }
}

/// A candidate knowledge claim emitted by an expert, with its verification
/// signals (the input the tiered VerifierAgent consumes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CandidateClaim {
    /// The claim to admit if it verifies.
    pub claim: Claim,
    /// The verification signals for the tiered check.
    pub signals: ClaimCandidate,
    /// `derived_from` premise parents to record on admission.
    pub premises: Vec<ClaimId>,
}

impl CandidateClaim {
    /// Construct a candidate claim with its verification signals and premises.
    #[must_use]
    pub fn new(claim: Claim, signals: ClaimCandidate, premises: Vec<ClaimId>) -> Self {
        Self {
            claim,
            signals,
            premises,
        }
    }
}

/// A candidate skill emitted by an expert, with its PAC evaluation statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CandidateSkill {
    /// The skill to admit if it certifies.
    pub skill: Skill,
    /// Empirical pass rate `p̂`.
    pub p_hat: f64,
    /// Number of evaluation tasks `m`.
    pub m: u32,
    /// Confidence parameter `δ`.
    pub delta: f64,
}

impl CandidateSkill {
    /// Construct a candidate skill with its PAC evaluation statistics.
    #[must_use]
    pub fn new(skill: Skill, p_hat: f64, m: u32, delta: f64) -> Self {
        Self {
            skill,
            p_hat,
            m,
            delta,
        }
    }
}

/// One expert's output for a query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExpertOutput {
    /// Opaque answer key, used by the critic for self-consistency `p̂`.
    pub answer_key: u64,
    /// Candidate claims.
    pub claims: Vec<CandidateClaim>,
    /// Candidate skills.
    pub skills: Vec<CandidateSkill>,
}

impl ExpertOutput {
    /// Construct an expert output.
    #[must_use]
    pub fn new(answer_key: u64, claims: Vec<CandidateClaim>, skills: Vec<CandidateSkill>) -> Self {
        Self {
            answer_key,
            claims,
            skills,
        }
    }
}

/// A frozen frontier expert (paper Definition 1). Generation is the only
/// behavior; weights never change — all improvement accrues in the store.
pub trait Expert {
    /// The expert's model identifier.
    fn id(&self) -> ModelId;
    /// The expert's provider family (used by the Tier-2 cross-family quorum).
    fn family(&self) -> String;
    /// Generate candidates for a query given the retrieved context.
    fn generate(&self, query: &Query, context: &PackedContext) -> ExpertOutput;
}

/// Engine configuration (paper Appendix B `EngineConfig`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EngineConfig {
    /// Maximum cycles `Bmax`.
    pub budget: u32,
    /// Convergence tolerance `ε`.
    pub eps: f64,
    /// Convergence patience `k`.
    pub patience: u32,
    /// Drift threshold `τ`.
    pub tau_gdi: f64,
    /// Soft fan-out `m`.
    pub fanout: usize,
    /// UCB near-optimality margin `γ`.
    pub gamma: f64,
    /// Budget weight `λ$`.
    pub lambda_dollar: f64,
    /// Budget weight `λℓ`.
    pub lambda_latency: f64,
    /// Skill competence threshold `τs`.
    pub tau_skill: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            budget: 50,
            eps: 1e-3,
            patience: 3,
            tau_gdi: 0.5,
            fanout: 3,
            gamma: 0.03,
            lambda_dollar: 0.15,
            lambda_latency: 0.05,
            tau_skill: 0.8,
        }
    }
}

/// The per-cycle inputs the host supplies (curriculum batch + drift signal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CycleInput {
    /// The frontier task batch `Qₜ` for this cycle.
    pub queries: Vec<Query>,
    /// The measured drift components for this cycle (CriticAgent).
    pub drift: DriftComponents,
    /// Whether all critical constraints hold (`CPS = 1`).
    pub critical_ok: bool,
}

impl CycleInput {
    /// Construct a cycle input.
    #[must_use]
    pub fn new(queries: Vec<Query>, drift: DriftComponents, critical_ok: bool) -> Self {
        Self {
            queries,
            drift,
            critical_ok,
        }
    }
}

/// The outcome of one cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CycleReport {
    /// Cycle index `t`.
    pub cycle: u32,
    /// Admitted mass `µ(Aₜ)` (counting measure: claims + skills admitted).
    pub admitted_mass: f64,
    /// The combined GDI value for this cycle.
    pub gdi: f64,
    /// Whether the drift gate forced a rollback.
    pub rolled_back: bool,
    /// Whether convergence was declared after this cycle.
    pub converged: bool,
    /// Live synergy count (Theorem 2(b)) after this cycle.
    pub synergy_count: usize,
}

/// The RSI Loop Engine. Generic over the [`KnowledgeStore`] so the in-memory
/// reference and the live `gauss-memory` backend drive it identically.
pub struct RsiEngine<S: KnowledgeStore> {
    store: S,
    router: LinUcbRouter,
    experts: Vec<Box<dyn Expert>>,
    cfg: EngineConfig,
    drift_gate: DriftGate,
    dualrag: DualRagParams,
    verifier: VerifierConfig,
    cycle: u32,
    last_checkpoint: SnapshotId,
    low_admit_streak: u32,
    /// Minimum admitting tier; tightened after a drift rollback (lowers `δv`).
    min_tier: u8,
    events: Vec<CycleEvent>,
    rng: Lcg,
}

impl<S: KnowledgeStore> RsiEngine<S> {
    /// Construct an engine. The router's arm count must equal the number of
    /// experts.
    ///
    /// # Panics
    /// Panics if `router.arms() != experts.len()`.
    #[must_use]
    pub fn new(
        store: S,
        router: LinUcbRouter,
        experts: Vec<Box<dyn Expert>>,
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
            rng: Lcg::new(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// Borrow the underlying store (for inspecting the accrued state).
    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// The events emitted so far (consumed by the Web/TUI surfaces).
    #[must_use]
    pub fn events(&self) -> &[CycleEvent] {
        &self.events
    }

    /// The current cycle index.
    #[must_use]
    pub const fn cycle(&self) -> u32 {
        self.cycle
    }

    /// Process one query: route → retrieve → generate → critique → verify,
    /// accumulating admissions into `batch` and the per-arm `emitted`/
    /// `admitted` tallies. Steps 1–5 of Algorithm 1's inner loop.
    fn process_query(
        &mut self,
        q: &Query,
        t: u32,
        emitted: &mut [u32],
        admitted: &mut [u32],
        batch: &mut AdmitBatch,
    ) {
        // 1. Route (Algorithm 3) with the exploration floor.
        let draw = self.rng.next_f64();
        let explore_arm = self.rng.next_index(self.experts.len());
        let dispatch = self.router.route(
            &q.context_features,
            self.cfg.fanout,
            self.cfg.gamma,
            draw,
            explore_arm,
        );
        // 2. DualRAG retrieval.
        let ctx = retrieve(&self.store, &q.embedding, &q.seeds, &self.dualrag);
        // 3. Generate with the selected experts; 4. critique (p̂); 5. verify.
        let mut answer_keys: Vec<u64> = Vec::new();
        for aw in &dispatch.arms {
            let Some(expert) = self.experts.get(aw.arm) else {
                continue;
            };
            let out = expert.generate(q, &ctx);
            answer_keys.push(out.answer_key);
            if let Some(e) = emitted.get_mut(aw.arm) {
                *e = e.saturating_add(saturating_u32(
                    out.claims.len().saturating_add(out.skills.len()),
                ));
            }
            for cand in out.claims {
                let verdict = verify_claim(&cand.signals, &self.verifier);
                if let crate::verify::Verdict::Pass { tier } = verdict {
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
        let _p_hat = self_consistency(&answer_keys);
    }

    /// Run one cycle of Φ (Algorithm 1).
    pub fn run_cycle(&mut self, input: &CycleInput) -> CycleReport {
        let t = self.cycle;
        self.events.push(CycleEvent::Started { t });

        // Per-arm tally of (emitted, admitted) for the post-verification reward.
        let mut emitted = vec![0u32; self.experts.len()];
        let mut admitted = vec![0u32; self.experts.len()];
        let mut batch = AdmitBatch::default();

        for q in &input.queries {
            self.process_query(q, t, &mut emitted, &mut admitted, &mut batch);
        }

        let admitted_mass = saturating_u32(batch.claims.len().saturating_add(batch.skills.len()));
        #[allow(clippy::cast_lossless)]
        let admitted_mass_f = f64::from(admitted_mass);
        let gdi = input.drift.gdi(&self.drift_gate.weights);

        // 6. Drift / constraint gate (paper Eq. 17; SAHOO).
        if self.drift_gate.evaluate(&input.drift, input.critical_ok) == DriftVerdict::Rollback {
            let dropped = self.store.rollback(self.last_checkpoint);
            self.min_tier = self.min_tier.saturating_sub(1).max(1); // tighten tier
            self.events.push(CycleEvent::Drift {
                drift: input.drift,
                gdi,
            });
            self.events.push(CycleEvent::RolledBack {
                to: self.last_checkpoint.0,
            });
            let report = CycleReport {
                cycle: t,
                admitted_mass: 0.0,
                gdi,
                rolled_back: true,
                converged: false,
                synergy_count: self.store.synergy_count(),
            };
            // A rolled-back cycle still advances the index but admits nothing.
            self.cycle = self.cycle.saturating_add(1);
            let _ = dropped;
            return report;
        }

        // 7. Admit (Φ) + checkpoint.
        self.store.admit(batch);
        self.last_checkpoint = self.store.checkpoint(t, &format!("cycle-{t}"));
        self.events.push(CycleEvent::Admitted {
            mass: admitted_mass_f,
        });
        self.events.push(CycleEvent::Drift {
            drift: input.drift,
            gdi,
        });

        // 8. Router reward update (post-verification, Eq. 4).
        self.update_rewards(&input.queries, &emitted, &admitted);

        // 9. Convergence detector (Theorem 1 stopping rule).
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
            synergy_count: self.store.synergy_count(),
        }
    }

    /// Run cycles until convergence, the budget `Bmax`, or a safety halt,
    /// pulling each cycle's input from `next` (Proposition 3 termination).
    /// Returns the per-cycle reports.
    pub fn run<F>(&mut self, mut next: F) -> Vec<CycleReport>
    where
        F: FnMut(u32) -> CycleInput,
    {
        let mut reports = Vec::new();
        while self.cycle < self.cfg.budget {
            let input = next(self.cycle);
            let report = self.run_cycle(&input);
            let converged = report.converged;
            reports.push(report);
            if converged {
                break;
            }
        }
        reports
    }

    fn update_rewards(&mut self, queries: &[Query], emitted: &[u32], admitted: &[u32]) {
        // Average context over the batch as the reward feature (the bandit
        // optimizes certified utility per Eq. 4; costs default to zero here).
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
            let reward = cost_adjusted_reward(
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

/// Saturating `usize -> u32` for counting masses.
pub(crate) fn saturating_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A tiny deterministic LCG so exploration draws are reproducible (the engine
/// core is RNG-free in spirit: the same seed replays identically).
#[derive(Debug, Clone)]
pub(crate) struct Lcg {
    state: u64,
}

impl Lcg {
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
    }

    pub(crate) fn next_f64(&mut self) -> f64 {
        self.step();
        #[allow(clippy::cast_precision_loss)]
        let mantissa = (self.state >> 11) as f64;
        mantissa / (9_007_199_254_740_992.0_f64) // 2^53
    }

    pub(crate) fn next_index(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        self.step();
        let m = u64::try_from(n).unwrap_or(1);
        usize::try_from(self.state.checked_rem(m).unwrap_or(0)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdi::DriftWeights;
    use crate::kg::{ClaimStatus, Provenance};
    use crate::router::LinUcbRouter;
    use crate::state::SkillId;

    /// An expert that emits a fixed, always-verifiable Tier-1 claim per call,
    /// with provenance spanning its own family plus a partner family (so the
    /// admitted item is synergistic).
    struct StubExpert {
        family: String,
        next_id: std::cell::Cell<u64>,
        base: u64,
    }

    impl StubExpert {
        fn new(family: &str, base: u64) -> Self {
            Self {
                family: family.to_owned(),
                next_id: std::cell::Cell::new(base),
                base,
            }
        }
    }

    impl Expert for StubExpert {
        fn id(&self) -> ModelId {
            ModelId(format!("stub/{}", self.family))
        }
        fn family(&self) -> String {
            self.family.clone()
        }
        fn generate(&self, _q: &Query, _ctx: &PackedContext) -> ExpertOutput {
            let id = self.next_id.get();
            self.next_id.set(id.saturating_add(1000));
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
                answer_key: self.base,
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

    fn engine() -> RsiEngine<crate::kg::InMemoryKnowledgeStore> {
        let experts: Vec<Box<dyn Expert>> = vec![
            Box::new(StubExpert::new("openai", 1)),
            Box::new(StubExpert::new("anthropic", 2)),
        ];
        let router = LinUcbRouter::new(2, 2, 0.6, 0.05);
        RsiEngine::new(
            crate::kg::InMemoryKnowledgeStore::new(),
            router,
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

    #[test]
    fn a_productive_cycle_admits_and_accrues_state() {
        let mut e = engine();
        let report = e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        });
        assert!(report.admitted_mass > 0.0);
        assert!(!report.rolled_back);
        assert!(e.store().verified_claim_count() > 0);
    }

    #[test]
    fn high_drift_forces_rollback_and_admits_nothing() {
        let mut e = engine();
        // First, a clean cycle to establish a checkpoint.
        e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        });
        let before = e.store().verified_claim_count();
        // Now a high-drift cycle: must roll back, admit nothing.
        let report = e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.9, 0.9, 0.9, 0.9),
            critical_ok: true,
        });
        assert!(report.rolled_back);
        assert!((report.admitted_mass).abs() < 1e-12);
        // Rollback dropped anything admitted after the checkpoint.
        assert!(e.store().verified_claim_count() <= before);
    }

    #[test]
    fn critical_constraint_violation_rolls_back() {
        let mut e = engine();
        let report = e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: false,
        });
        assert!(report.rolled_back);
    }

    #[test]
    fn admitted_items_are_synergistic() {
        let mut e = engine();
        e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        });
        // The stub claims span two model families => counted as synergy.
        assert!(e.store().synergy_count() > 0);
    }

    #[test]
    fn loop_terminates_at_budget() {
        let mut e = engine();
        e.cfg.budget = 5;
        let reports = e.run(|_t| CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        });
        assert!(reports.len() <= 5);
        // Each productive cycle advanced the index.
        assert!(e.cycle() <= 5);
    }

    #[test]
    fn emits_lifecycle_events() {
        let mut e = engine();
        e.run_cycle(&CycleInput {
            queries: vec![query()],
            drift: DriftComponents::new(0.0, 0.0, 0.0, 0.0),
            critical_ok: true,
        });
        assert!(e
            .events()
            .iter()
            .any(|ev| matches!(ev, CycleEvent::Started { .. })));
        assert!(e
            .events()
            .iter()
            .any(|ev| matches!(ev, CycleEvent::Admitted { .. })));
        let _ = SkillId(0); // keep the import meaningful for future skill tests
    }
}
