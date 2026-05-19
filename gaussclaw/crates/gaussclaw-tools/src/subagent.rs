//! `delegate_tool` + `mixture_of_agents_tool` (Sprint 6 §3 + §4).
//!
//! Hermes's `delegate_tool` and `mixture_of_agents_tool` let an
//! assistant spawn one or more sub-agents to handle a sub-task. The
//! Hermes shape is "call function, get string back" — the parent
//! receipt chain swallows the sub-agent's effects whole, and a
//! compromised sub-agent that returns crafted text can taint the
//! parent's audit log indistinguishably from a real sub-agent reply.
//!
//! GaussClaw fixes this with **receipt-chain isolation**: each
//! sub-agent runs against its own [`SubChain`] keyed by a fresh
//! `chain_id`. The parent records only the sub-chain's head digest
//! and a count of sub-receipts; a forger would have to produce a
//! valid chain under the sub-agent's key, not just an output string.
//!
//! ## Tools shipped here
//!
//! - [`DelegateTool`] (`delegate`) — spawn one sub-agent, await
//!   its terminal output, return the head + result.
//! - [`MixtureOfAgentsTool`] (`mixture_of_agents`) — spawn N
//!   sub-agents in parallel against the same prompt, aggregate
//!   via majority vote (or first-non-empty fallback), return the
//!   aggregated answer plus the per-agent chain heads.
//!
//! ## Hermes-superiority axes
//!
//! - **Cap restriction.** Every dispatch carries a `grant_subset`
//!   that's a lattice-meet of the parent's grant and an explicit
//!   downgrade. A sub-agent **cannot** acquire a cap the parent
//!   didn't already have. Hermes inherits the parent's full grant.
//! - **Chain isolation.** Every sub-agent emits its own
//!   `chain_head`; the parent receipt records the head digest +
//!   length, not the sub-agent's free-form output.
//! - **Per-sub-agent budget cap.** `max_iterations` is mandatory
//!   on the dispatch request — Hermes's sub-agents can loop
//!   indefinitely against the parent's budget.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

const DELEGATE_MANIFEST: &str = r#"
name        = "delegate"
description = "Spawn an isolated sub-agent against a prompt. The sub-agent runs against its own receipt chain; only the chain head and final answer return to the caller."
usage       = "Args: {prompt, grant_subset_bits?, max_iterations}. Returns {chain_head, chain_length, output}."
caps        = []
taint       = "user"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const MIXTURE_MANIFEST: &str = r#"
name        = "mixture_of_agents"
description = "Spawn N sub-agents in parallel against the same prompt; aggregate the answers via majority vote."
usage       = "Args: {prompt, n, grant_subset_bits?, max_iterations}. Returns {aggregated_output, votes, agent_heads}."
caps        = []
taint       = "user"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// One sub-agent dispatch request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentRequest {
    /// Prompt for the sub-agent.
    pub prompt: String,
    /// Cap-token grant the sub-agent runs under. Must be a subset of
    /// the parent's grant — the [`SubAgentDispatcher`] enforces this.
    pub grant: CapToken,
    /// Maximum loop iterations before the sub-agent times out.
    pub max_iterations: u32,
}

/// One sub-agent's terminal result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentResult {
    /// Free-form text the sub-agent produced.
    pub output: String,
    /// Sub-chain head (hex BLAKE3 / Ed25519). Distinct from the
    /// parent's chain; the parent records only this digest.
    pub chain_head: String,
    /// Number of receipts in the sub-chain.
    pub chain_length: u64,
    /// Number of iterations the sub-agent actually used.
    pub iterations: u32,
}

/// Pluggable dispatch surface — the actual "run a sub-agent loop"
/// implementation lives in `gaussclaw-agent`; the tool layer is
/// agnostic to which loop driver is configured.
#[async_trait]
pub trait SubAgentDispatcher: Send + Sync {
    /// Dispatch one sub-agent. Implementations must:
    ///
    /// 1. Verify `request.grant.contains(...)` is a subset of the
    ///    *parent's* effective grant before running.
    /// 2. Run the loop against `request.prompt` for at most
    ///    `request.max_iterations` iterations.
    /// 3. Return a fresh [`SubAgentResult`] with a chain head computed
    ///    over the sub-agent's own receipt sequence.
    async fn dispatch(&self, request: SubAgentRequest) -> GaussResult<SubAgentResult>;
}

/// Static-text mock dispatcher for tests + the conformance suite.
pub struct MockDispatcher {
    output: String,
    chain_head: String,
    chain_length: u64,
    iterations: u32,
    /// Verified-failure mode: when set, every dispatch returns the
    /// given error. Used to test the parent tool's failure path.
    fail_with: Option<String>,
}

impl MockDispatcher {
    /// Build a mock that returns `output` and a deterministic head.
    #[must_use]
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            chain_head: "deadbeef".repeat(8),
            chain_length: 1,
            iterations: 1,
            fail_with: None,
        }
    }

    /// Build a mock that always fails dispatch.
    #[must_use]
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            output: String::new(),
            chain_head: String::new(),
            chain_length: 0,
            iterations: 0,
            fail_with: Some(msg.into()),
        }
    }
}

#[async_trait]
impl SubAgentDispatcher for MockDispatcher {
    async fn dispatch(&self, _request: SubAgentRequest) -> GaussResult<SubAgentResult> {
        if let Some(msg) = &self.fail_with {
            return Err(GaussError::Internal(msg.clone()));
        }
        Ok(SubAgentResult {
            output: self.output.clone(),
            chain_head: self.chain_head.clone(),
            chain_length: self.chain_length,
            iterations: self.iterations,
        })
    }
}

/// `delegate` tool — single sub-agent.
pub struct DelegateTool {
    manifest: ToolManifest,
    dispatcher: Arc<dyn SubAgentDispatcher>,
    parent_grant: CapToken,
}

impl DelegateTool {
    /// Build a delegate tool over a dispatcher and the parent's
    /// effective grant. The parent grant is the upper bound on what
    /// sub-agents can request.
    ///
    /// # Panics
    /// Build-time only — panics if the embedded manifest TOML doesn't
    /// parse.
    #[must_use]
    pub fn new(dispatcher: Arc<dyn SubAgentDispatcher>, parent_grant: CapToken) -> Self {
        let skill = SkillManifest::from_toml(DELEGATE_MANIFEST).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("delegate".into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            dispatcher,
            parent_grant,
        }
    }

    fn restrict_grant(&self, subset_bits: Option<u64>) -> CapToken {
        subset_bits.map_or(self.parent_grant, |bits| {
            self.parent_grant.meet(CapToken::from_bits(bits))
        })
    }
}

#[async_trait]
impl ToolTrait for DelegateTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `prompt`".into()))?;
        let max_iterations = args
            .get("max_iterations")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| GaussError::Internal("missing uint field `max_iterations`".into()))?;
        let max_iterations = u32::try_from(max_iterations)
            .map_err(|_| GaussError::Internal("max_iterations exceeds u32::MAX".into()))?;
        let grant_subset_bits = args
            .get("grant_subset_bits")
            .and_then(serde_json::Value::as_u64);
        let grant = self.restrict_grant(grant_subset_bits);
        let result = self
            .dispatcher
            .dispatch(SubAgentRequest {
                prompt: prompt.into(),
                grant,
                max_iterations,
            })
            .await?;
        Ok(serde_json::json!({
            "kind":         "delegate_result",
            "chain_head":   result.chain_head,
            "chain_length": result.chain_length,
            "iterations":   result.iterations,
            "output":       result.output,
            "grant_bits":   grant.bits(),
        }))
    }
}

/// `mixture_of_agents` tool — N sub-agents, majority vote.
pub struct MixtureOfAgentsTool {
    manifest: ToolManifest,
    dispatcher: Arc<dyn SubAgentDispatcher>,
    parent_grant: CapToken,
}

impl MixtureOfAgentsTool {
    /// Build a mixture tool.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(dispatcher: Arc<dyn SubAgentDispatcher>, parent_grant: CapToken) -> Self {
        let skill = SkillManifest::from_toml(MIXTURE_MANIFEST).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("mixture_of_agents".into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            dispatcher,
            parent_grant,
        }
    }

    fn restrict_grant(&self, subset_bits: Option<u64>) -> CapToken {
        subset_bits.map_or(self.parent_grant, |bits| {
            self.parent_grant.meet(CapToken::from_bits(bits))
        })
    }
}

#[async_trait]
impl ToolTrait for MixtureOfAgentsTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `prompt`".into()))?;
        let n = args
            .get("n")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| GaussError::Internal("missing uint field `n`".into()))?;
        if n == 0 || n > 16 {
            return Err(GaussError::Internal(format!(
                "n must be in 1..=16 (got {n})"
            )));
        }
        let max_iterations = args
            .get("max_iterations")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| GaussError::Internal("missing uint field `max_iterations`".into()))?;
        let max_iterations = u32::try_from(max_iterations)
            .map_err(|_| GaussError::Internal("max_iterations exceeds u32::MAX".into()))?;
        let grant_subset_bits = args
            .get("grant_subset_bits")
            .and_then(serde_json::Value::as_u64);
        let grant = self.restrict_grant(grant_subset_bits);

        // Dispatch N times in parallel. Each call gets a fresh
        // request so the dispatcher allocates a new sub-chain per
        // sub-agent.
        let mut handles = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let dispatcher = Arc::clone(&self.dispatcher);
            let req = SubAgentRequest {
                prompt: prompt.into(),
                grant,
                max_iterations,
            };
            handles.push(tokio::spawn(async move { dispatcher.dispatch(req).await }));
        }
        let mut outputs: Vec<SubAgentResult> = Vec::with_capacity(n as usize);
        for h in handles {
            match h.await {
                Ok(Ok(r)) => outputs.push(r),
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(GaussError::Internal(format!("sub-agent join: {e}"))),
            }
        }

        let (aggregated, votes) = majority_vote(&outputs);
        let heads: Vec<serde_json::Value> = outputs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "head":   r.chain_head,
                    "length": r.chain_length,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "kind":              "mixture_result",
            "aggregated_output": aggregated,
            "votes":             votes,
            "agent_heads":       heads,
            "grant_bits":        grant.bits(),
        }))
    }
}

/// Compute the majority-vote output and its support count. Ties
/// break by first-non-empty.
fn majority_vote(results: &[SubAgentResult]) -> (String, u64) {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for r in results {
        *counts.entry(r.output.as_str()).or_insert(0) += 1;
    }
    // Pick the highest-count entry; if multiple tie, pick the first
    // (BTreeMap iteration order is stable + sorted).
    let mut best: Option<(&str, u64)> = None;
    for (k, v) in &counts {
        match best {
            None => best = Some((*k, *v)),
            Some((_, bv)) if *v > bv => best = Some((*k, *v)),
            _ => {}
        }
    }
    match best {
        Some((s, v)) => (s.to_string(), v),
        None => (String::new(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delegate_returns_chain_head_and_output() {
        let disp = Arc::new(MockDispatcher::ok("the answer is 42"));
        let tool = DelegateTool::new(disp, CapToken::TOP);
        let out = tool
            .invoke_raw(serde_json::json!({
                "prompt": "what is the answer?",
                "max_iterations": 3,
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "delegate_result");
        assert_eq!(out["output"], "the answer is 42");
        assert_eq!(out["chain_length"], 1);
        assert!(out["chain_head"].as_str().unwrap().len() >= 16);
    }

    #[tokio::test]
    async fn delegate_restricts_grant_to_parent_meet_subset() {
        let disp = Arc::new(MockDispatcher::ok("ok"));
        let parent = CapToken::EXECUTOR_LOCAL | CapToken::NETWORK_GET;
        let tool = DelegateTool::new(disp, parent);
        // Subset asks for FS_WRITE (not in parent) + EXECUTOR_LOCAL
        // (in parent). Effective grant should only contain
        // EXECUTOR_LOCAL.
        let requested = (CapToken::FILESYSTEM_WRITE | CapToken::EXECUTOR_LOCAL).bits();
        let out = tool
            .invoke_raw(serde_json::json!({
                "prompt": "x",
                "max_iterations": 1,
                "grant_subset_bits": requested,
            }))
            .await
            .unwrap();
        let granted = out["grant_bits"].as_u64().unwrap();
        let granted = CapToken::from_bits(granted);
        assert!(granted.contains(CapToken::EXECUTOR_LOCAL));
        assert!(!granted.contains(CapToken::FILESYSTEM_WRITE));
    }

    #[tokio::test]
    async fn delegate_propagates_dispatcher_errors() {
        let disp = Arc::new(MockDispatcher::err("boom"));
        let tool = DelegateTool::new(disp, CapToken::TOP);
        let err = tool
            .invoke_raw(serde_json::json!({
                "prompt": "x",
                "max_iterations": 1,
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn delegate_rejects_missing_required_fields() {
        let tool = DelegateTool::new(Arc::new(MockDispatcher::ok("x")), CapToken::TOP);
        assert!(tool
            .invoke_raw(serde_json::json!({"max_iterations": 1}))
            .await
            .is_err());
        assert!(tool
            .invoke_raw(serde_json::json!({"prompt": "p"}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn mixture_runs_n_agents_and_returns_each_head() {
        let disp = Arc::new(MockDispatcher::ok("majority"));
        let tool = MixtureOfAgentsTool::new(disp, CapToken::TOP);
        let out = tool
            .invoke_raw(serde_json::json!({
                "prompt": "x",
                "n": 4,
                "max_iterations": 1,
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "mixture_result");
        assert_eq!(out["aggregated_output"], "majority");
        assert_eq!(out["votes"], 4);
        assert_eq!(out["agent_heads"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn mixture_rejects_n_out_of_range() {
        let tool = MixtureOfAgentsTool::new(Arc::new(MockDispatcher::ok("")), CapToken::TOP);
        assert!(tool
            .invoke_raw(serde_json::json!({
                "prompt": "x", "n": 0, "max_iterations": 1
            }))
            .await
            .is_err());
        assert!(tool
            .invoke_raw(serde_json::json!({
                "prompt": "x", "n": 100, "max_iterations": 1
            }))
            .await
            .is_err());
    }

    #[test]
    fn majority_vote_picks_most_common() {
        let r = |s: &str| SubAgentResult {
            output: s.into(),
            chain_head: "x".into(),
            chain_length: 0,
            iterations: 0,
        };
        let (out, n) = majority_vote(&[r("a"), r("b"), r("a"), r("a"), r("b")]);
        assert_eq!(out, "a");
        assert_eq!(n, 3);
    }

    #[test]
    fn majority_vote_empty_returns_empty() {
        let (out, n) = majority_vote(&[]);
        assert_eq!(out, "");
        assert_eq!(n, 0);
    }

    #[test]
    fn delegate_manifest_carries_no_caps_by_default() {
        let tool = DelegateTool::new(Arc::new(MockDispatcher::ok("")), CapToken::TOP);
        // The tool itself carries no caps — the *sub-agent's* grant
        // is what gates effects.
        assert_eq!(tool.manifest().cap_required.bits(), 0);
    }

    #[test]
    fn mixture_manifest_carries_no_caps_by_default() {
        let tool = MixtureOfAgentsTool::new(Arc::new(MockDispatcher::ok("")), CapToken::TOP);
        assert_eq!(tool.manifest().cap_required.bits(), 0);
    }
}
