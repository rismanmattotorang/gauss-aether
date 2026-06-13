//! `ProviderExpert` — a live-provider-backed [`AsyncExpert`].
//!
//! Wraps any [`gaussclaw_agent::ProviderHandle`] (the OpenRouter driver, a
//! vendor driver, or a test stub) as a frozen frontier expert in the RSI loop.
//! The expert prompts the model to emit candidate knowledge as JSON, parses
//! that into verifiable [`gauss_rsi::engine::CandidateClaim`]s, and tags each
//! with provenance — so the model's free-text output becomes auditable,
//! verifier-gated knowledge.
//!
//! The expected model output is a JSON object:
//!
//! ```json
//! { "claims": [ { "content": "...", "executable": true, "passes": true,
//!                 "cites_sources": false, "families": ["openai","anthropic"] } ],
//!   "skills": [ { "name": "...", "pass_rate": 0.95, "m_tests": 200 } ] }
//! ```
//!
//! Output that does not parse yields no admissions (the model gets no credit
//! for unverifiable text) — a fail-closed default.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_rsi::dualrag::PackedContext;
use gauss_rsi::engine::{CandidateClaim, CandidateSkill, ExpertOutput, Query};
use gauss_rsi::kg::{Claim, ClaimStatus, ModelId, Provenance, Skill};
use gauss_rsi::state::{ClaimId, SkillId};
use gauss_rsi::verify::ClaimCandidate;
use gauss_rsi::AsyncExpert;
use gaussclaw_agent::{Message, Prompt, ProviderHandle};
use serde::Deserialize;

/// Embedding dimension used for content vectors (kept small; the production
/// path uses the model's native embeddings).
const EMBED_DIM: usize = 16;

/// The default system instruction asking the model for structured claims.
const SYSTEM_PROMPT: &str =
    "You are a knowledge-composition expert. Given the query and retrieved \
context, emit a JSON object {\"claims\":[{\"content\":string,\"executable\":bool,\"passes\":bool,\
\"cites_sources\":bool,\"families\":[string]}],\"skills\":[{\"name\":string,\"pass_rate\":number,\
\"m_tests\":int}]}. Emit only claims you can justify from the context.";

/// A frozen frontier expert backed by a live provider.
pub struct ProviderExpert {
    provider: Arc<dyn ProviderHandle>,
    slug: String,
    family: String,
}

impl ProviderExpert {
    /// Wrap a provider as an expert dispatching to `slug` (an OpenRouter model
    /// id) of provider `family`.
    #[must_use]
    pub fn new(
        provider: Arc<dyn ProviderHandle>,
        slug: impl Into<String>,
        family: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            slug: slug.into(),
            family: family.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawOutput {
    #[serde(default)]
    claims: Vec<RawClaim>,
    #[serde(default)]
    skills: Vec<RawSkill>,
}

#[derive(Debug, Deserialize)]
struct RawClaim {
    content: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    passes: bool,
    #[serde(default)]
    cites_sources: bool,
    #[serde(default)]
    families: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawSkill {
    #[serde(default)]
    name: String,
    #[serde(default)]
    pass_rate: f64,
    #[serde(default)]
    m_tests: u32,
}

/// Deterministic FNV-1a hash → stable id for content (so identical claims
/// dedup in the store rather than accumulating duplicate rows).
fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for byte in s.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A small deterministic content embedding (byte-bucket histogram, L2-ish).
fn embed(s: &str) -> Vec<f32> {
    let mut v = vec![0.0_f32; EMBED_DIM];
    for byte in s.as_bytes() {
        let idx = (*byte as usize) % EMBED_DIM;
        if let Some(slot) = v.get_mut(idx) {
            *slot += 1.0;
        }
    }
    v
}

impl ProviderExpert {
    fn build_prompt(&self, query: &Query, context: &PackedContext) -> Prompt {
        let ctx_line = format!(
            "Retrieved context: {} premise items, {} total items.",
            context.premise_count,
            context.items.len()
        );
        Prompt::new(
            self.slug.clone(),
            vec![
                Message::new("system", SYSTEM_PROMPT),
                Message::new("user", format!("Query #{}. {ctx_line}", query.id)),
            ],
        )
    }

    fn parse(&self, text: &str, query_id: u64) -> ExpertOutput {
        let Ok(raw) = serde_json::from_str::<RawOutput>(text) else {
            return ExpertOutput::default();
        };
        let mut claims = Vec::new();
        for rc in raw.claims {
            let id = fnv1a(&format!("{}|{}", self.slug, rc.content));
            let mut families: std::collections::BTreeSet<String> =
                rc.families.into_iter().collect();
            families.insert(self.family.clone());
            let embedding = embed(&rc.content);
            let provenance =
                Provenance::new(vec![ModelId(self.slug.clone())], families, Vec::new(), 0, 0);
            let claim = Claim::new(
                ClaimId(id),
                rc.content,
                embedding,
                0.9,
                ClaimStatus::Verified,
                provenance,
            );
            let signals = ClaimCandidate::new(
                rc.executable,
                rc.passes,
                rc.cites_sources,
                Vec::new(),
                false,
                false,
            );
            claims.push(CandidateClaim::new(claim, signals, Vec::new()));
        }
        let skills = raw
            .skills
            .into_iter()
            .map(|rs| {
                let skill = Skill::new(
                    SkillId(fnv1a(&format!("{}|{}", self.slug, rs.name))),
                    rs.name,
                    String::new(),
                    String::new(),
                    String::new(),
                    Vec::new(),
                    rs.pass_rate,
                    0.0,
                    rs.m_tests,
                    0,
                );
                CandidateSkill::new(skill, rs.pass_rate, rs.m_tests, 0.05)
            })
            .collect();
        ExpertOutput::new(fnv1a(&self.slug).wrapping_add(query_id), claims, skills)
    }
}

#[async_trait]
impl AsyncExpert for ProviderExpert {
    fn id(&self) -> ModelId {
        ModelId(self.slug.clone())
    }

    fn family(&self) -> String {
        self.family.clone()
    }

    async fn generate(&self, query: &Query, context: &PackedContext) -> ExpertOutput {
        let prompt = self.build_prompt(query, context);
        match self.provider.complete(&prompt).await {
            Ok(completion) => self.parse(&completion.text, query.id),
            Err(e) => {
                tracing::warn!(error = %e, slug = %self.slug, "expert generation failed");
                ExpertOutput::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_agent::{Completion, ProviderResult, TokenCount};

    struct JsonProvider(String);

    #[async_trait]
    impl ProviderHandle for JsonProvider {
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "json-stub"
        }
        async fn complete(&self, _p: &Prompt) -> ProviderResult<Completion> {
            Ok(Completion::new(
                self.0.clone(),
                "stub",
                "stop",
                TokenCount::new(1, 1),
            ))
        }
    }

    fn query() -> Query {
        Query::new(7, vec![1.0; EMBED_DIM], Vec::new(), vec![1.0, 0.0])
    }

    #[tokio::test]
    async fn parses_json_claims_into_candidates() {
        let json = r#"{"claims":[{"content":"2+2=4","executable":true,"passes":true,"families":["openai","anthropic"]}]}"#;
        let expert = ProviderExpert::new(
            Arc::new(JsonProvider(json.to_owned())),
            "openai/gpt-4o",
            "openai",
        );
        let out = expert
            .generate(&query(), &PackedContext::new(Vec::new(), 0))
            .await;
        assert_eq!(out.claims.len(), 1);
        let c = &out.claims[0];
        assert!(c.signals.tier1_checkable && c.signals.tier1_passes);
        // Families include the listed two plus the expert's own — synergistic.
        assert!(c.claim.provenance.model_families.len() >= 2);
    }

    #[tokio::test]
    async fn unparseable_output_yields_nothing() {
        let expert = ProviderExpert::new(
            Arc::new(JsonProvider("not json".to_owned())),
            "openai/gpt-4o",
            "openai",
        );
        let out = expert
            .generate(&query(), &PackedContext::new(Vec::new(), 0))
            .await;
        assert!(out.claims.is_empty() && out.skills.is_empty());
    }

    #[tokio::test]
    async fn parses_skills_with_pac_stats() {
        let json = r#"{"skills":[{"name":"sort","pass_rate":0.96,"m_tests":500}]}"#;
        let expert = ProviderExpert::new(
            Arc::new(JsonProvider(json.to_owned())),
            "deepseek/r1",
            "deepseek",
        );
        let out = expert
            .generate(&query(), &PackedContext::new(Vec::new(), 0))
            .await;
        assert_eq!(out.skills.len(), 1);
        assert_eq!(out.skills[0].m, 500);
    }
}
