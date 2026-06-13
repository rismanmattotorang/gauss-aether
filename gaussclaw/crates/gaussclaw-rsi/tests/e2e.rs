//! End-to-end validation of the live Gauss-Agent0 wiring.
//!
//! Drives [`gauss_rsi::AsyncRsiEngine`] against the live SurrealDB-backed
//! [`gaussclaw_rsi::SurrealKnowledgeStore`] with [`gaussclaw_rsi::ProviderExpert`]s
//! wrapping in-process providers. Validates the full self-improvement loop:
//! routed generation → DualRAG retrieval → tiered verification → admission to
//! the live store → drift rollback → geometric convergence.

#![allow(clippy::doc_markdown)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gauss_rsi::dualrag::DualRagParams;
use gauss_rsi::engine::{CycleInput, EngineConfig, Query};
use gauss_rsi::gdi::{DriftComponents, DriftGate, DriftWeights};
use gauss_rsi::live::AsyncExpert;
use gauss_rsi::router::LinUcbRouter;
use gauss_rsi::verify::VerifierConfig;
use gauss_rsi::{AsyncKnowledgeStore, AsyncRsiEngine};
use gaussclaw_agent::{Completion, Prompt, ProviderHandle, ProviderResult, TokenCount};
use gaussclaw_rsi::{ProviderExpert, SurrealKnowledgeStore};

/// A provider that emits one synergistic Tier-1 claim per call for the first
/// `productive_cycles` calls, then empty output — so the loop saturates and
/// the convergence detector fires.
struct TaperingProvider {
    family: String,
    calls: AtomicU32,
    productive_cycles: u32,
}

#[async_trait]
impl ProviderHandle for TaperingProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "tapering"
    }
    async fn complete(&self, _p: &Prompt) -> ProviderResult<Completion> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let body = if n < self.productive_cycles {
            format!(
                r#"{{"claims":[{{"content":"fact-{}-{}","executable":true,"passes":true,"families":["openai","anthropic"]}}]}}"#,
                self.family, n
            )
        } else {
            "{}".to_owned()
        };
        Ok(Completion::new(body, "stub", "stop", TokenCount::new(1, 1)))
    }
}

fn experts(productive: u32) -> Vec<Box<dyn AsyncExpert>> {
    vec![
        Box::new(ProviderExpert::new(
            Arc::new(TaperingProvider {
                family: "openai".into(),
                calls: AtomicU32::new(0),
                productive_cycles: productive,
            }),
            "openai/gpt-4o",
            "openai",
        )),
        Box::new(ProviderExpert::new(
            Arc::new(TaperingProvider {
                family: "anthropic".into(),
                calls: AtomicU32::new(0),
                productive_cycles: productive,
            }),
            "anthropic/claude-sonnet-4.5",
            "anthropic",
        )),
    ]
}

fn query(id: u64) -> Query {
    Query::new(id, vec![1.0; 16], Vec::new(), vec![1.0, 0.0])
}

fn low_drift(queries: Vec<Query>) -> CycleInput {
    CycleInput::new(queries, DriftComponents::new(0.0, 0.0, 0.0, 0.0), true)
}

async fn engine(productive: u32) -> AsyncRsiEngine<SurrealKnowledgeStore> {
    let store = SurrealKnowledgeStore::open_in_memory().await.unwrap();
    let mut cfg = EngineConfig::default();
    cfg.budget = 12;
    cfg.patience = 2;
    cfg.fanout = 2;
    AsyncRsiEngine::new(
        store,
        LinUcbRouter::new(2, 2, 0.6, 0.05),
        experts(productive),
        cfg,
        DriftGate::new(DriftWeights::default(), 0.5),
        DualRagParams::default(),
        VerifierConfig::default(),
    )
}

#[tokio::test]
async fn loop_accrues_synergistic_state_in_the_live_store() {
    let mut e = engine(100).await;
    let r0 = e.run_cycle(&low_drift(vec![query(1)])).await;
    assert!(r0.admitted_mass > 0.0, "first cycle should admit");
    assert!(!r0.rolled_back);
    e.run_cycle(&low_drift(vec![query(2)])).await;
    // Distinct verified claims persisted in SurrealDB, spanning >=2 families.
    assert!(e.store().verified_claim_count().await >= 2);
    assert!(
        e.store().synergy_count().await >= 2,
        "admitted items are synergistic"
    );
}

#[tokio::test]
async fn high_drift_triggers_live_rollback() {
    let mut e = engine(100).await;
    e.run_cycle(&low_drift(vec![query(1)])).await;
    let report = e
        .run_cycle(&CycleInput::new(
            vec![query(2)],
            DriftComponents::new(0.9, 0.9, 0.9, 0.9),
            true,
        ))
        .await;
    assert!(report.rolled_back);
    assert!((report.admitted_mass).abs() < 1e-12);
}

#[tokio::test]
async fn loop_converges_once_experts_taper() {
    // Experts stop emitting after 2 productive cycles; with patience=2 the
    // convergence detector then fires (Theorem 1 stopping rule).
    let mut e = engine(2).await;
    let reports = e.run(low_drift_for).await;
    assert!(
        reports.iter().any(|r| r.converged),
        "loop should converge after experts taper; reports: {reports:?}"
    );
    // Terminated before exhausting the budget.
    assert!(reports.len() < 12);
}

fn low_drift_for(t: u32) -> CycleInput {
    low_drift(vec![query(u64::from(t).wrapping_add(1))])
}

#[tokio::test]
async fn rollback_then_resume_keeps_pre_checkpoint_state() {
    let mut e = engine(100).await;
    e.run_cycle(&low_drift(vec![query(1)])).await; // checkpoint at cycle 0
    let after_first = e.store().verified_claim_count().await;
    // High drift rolls back anything admitted after the last checkpoint.
    e.run_cycle(&CycleInput::new(
        vec![query(2)],
        DriftComponents::new(0.95, 0.95, 0.95, 0.95),
        true,
    ))
    .await;
    // The pre-checkpoint state survives the rollback.
    assert!(e.store().verified_claim_count().await >= after_first.saturating_sub(0));
    // And the loop can resume productively afterward.
    let resumed = e.run_cycle(&low_drift(vec![query(3)])).await;
    assert!(!resumed.rolled_back);
}
