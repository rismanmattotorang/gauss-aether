//! `gaussclaw-agent` — agent-loop glue + kernel handle for the surfaces.
//!
//! Phase 1 Tasks 9 + 10 of `GAUSSCLAW_ROADMAP.md`. Two responsibilities,
//! delivered incrementally:
//!
//! 1. **Kernel gate (this slice).** A shared [`KernelHandle`] that every
//!    surface (`gaussclaw-web`, `gaussclaw-surfaces`, channel adapters)
//!    consults before processing a request. The handle wraps an
//!    `Arc<dyn Kernel>` and exposes [`KernelHandle::admit`] +
//!    [`KernelHandle::plane_for`] so all surfaces share one
//!    capability/taint gate.
//!
//! 2. **Turn policy (later slice).** The Hermes `run_conversation` body
//!    lifted into a `Differential Turn Engine` policy — assembles a
//!    prompt, dispatches to the provider plane, parses tool calls,
//!    repeats until done, writes the turn record. Stubbed today; lands
//!    once `gaussclaw-providers` ships in Phase 4.

#![allow(clippy::doc_markdown)]

pub mod agent_loop;
pub mod audit;
pub mod compaction;

pub use agent_loop::{
    parse_inline_tool_calls, AgentLoop, LoopEvent, LoopOutcome, LoopSink, MemorySink, NoopSink,
    ToolCall, DEFAULT_MAX_ITERATIONS,
};

pub use compaction::{Compactor, CompactionRecord, WindowedCompactor, SUMMARY_PREFIX};

pub use audit::{
    blake3_hex, AuditEntry, AuditTrace, InboundRecord, OutboundRecord, PlaneLabel,
    TurnCompleteRecord, TurnStartRecord,
};

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, TaintLabel};
use gauss_kernel::{Plane, PrivilegedKernel};
use gauss_traits::Kernel;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── kernel handle ──────────────────────────────────────────────────────────

/// Shared handle every surface holds in its state.
///
/// Carries the [`Kernel`] implementation (so admit gates work) and a
/// [`PlaneSelector`] policy (so every surface routes through the right
/// scheduler plane). Cheap to `Clone` — internally `Arc<dyn Kernel>`.
#[derive(Clone)]
pub struct KernelHandle {
    inner: Arc<dyn Kernel>,
    selector: PlaneSelector,
}

impl KernelHandle {
    /// Wrap an existing kernel implementation.
    pub fn new(kernel: Arc<dyn Kernel>) -> Self {
        Self {
            inner: kernel,
            selector: PlaneSelector::default(),
        }
    }

    /// Convenience: build a permissive privileged kernel (`CapToken::TOP`)
    /// with the default declassification map. Use this for tests and for
    /// the Phase 1 demo binary, where no real grant pipeline exists yet.
    #[must_use]
    pub fn permissive() -> Self {
        Self::new(Arc::new(PrivilegedKernel::new(CapToken::TOP)))
    }

    /// Borrow the inner kernel — useful for code that wants to call
    /// [`Kernel::current_grant`] directly.
    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        &self.inner
    }

    /// Joint capability/taint admission check. Forwards to the underlying
    /// [`Kernel::admit`]; returns `GaussError::Denied` /
    /// `GaussError::TaintTooHigh` on failure.
    pub fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()> {
        self.inner.admit(required, taint)
    }

    /// Map a surface-side request descriptor to the scheduler plane that
    /// owns its budget pool. The mapping is data — replace
    /// [`PlaneSelector`] on the handle to customise per deployment.
    #[must_use]
    pub const fn plane_for(&self, req: SurfaceRequest) -> Plane {
        self.selector.plane_for(req)
    }

    /// Swap the [`PlaneSelector`] policy.
    #[must_use]
    pub const fn with_selector(mut self, selector: PlaneSelector) -> Self {
        self.selector = selector;
        self
    }
}

impl std::fmt::Debug for KernelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KernelHandle")
            .field("grant", &self.inner.current_grant())
            .field("selector", &self.selector)
            .finish()
    }
}

// ─── plane selector ─────────────────────────────────────────────────────────

/// Surface-side request descriptor — what the kernel uses to pick a plane.
///
/// Every variant has a canonical mapping to one of the three scheduler
/// planes (Conversation / Daemon / Approval). The mapping is structural:
/// user-synchronous traffic goes to Conversation, background turns go to
/// Daemon, human-in-the-loop round trips go to Approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceRequest {
    /// CLI / TUI / REST / WS / OAI-compat — all user-synchronous.
    UserSync,
    /// `/v1/chat/completions` and `/v1/completions` — SDK chat traffic.
    SdkChat,
    /// Messaging-gateway ingress (Slack, Discord, Telegram, …).
    Channel,
    /// Scheduled or daemon-launched turn (cron, background sweeps).
    Scheduled,
    /// Human-in-the-loop approval prompt.
    Approval,
}

/// Policy that maps a [`SurfaceRequest`] to a scheduler [`Plane`].
///
/// The default policy follows the roadmap exactly:
///
/// | request                | plane          |
/// |---|---|
/// | `UserSync`, `SdkChat`, `Channel` | `Conversation` |
/// | `Scheduled`            | `Daemon`       |
/// | `Approval`             | `Approval`     |
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlaneSelector;

impl Default for PlaneSelector {
    fn default() -> Self {
        Self
    }
}

impl PlaneSelector {
    /// Return the plane for a given request descriptor.
    #[must_use]
    pub const fn plane_for(&self, req: SurfaceRequest) -> Plane {
        match req {
            SurfaceRequest::UserSync | SurfaceRequest::SdkChat | SurfaceRequest::Channel => {
                Plane::Conversation
            }
            SurfaceRequest::Scheduled => Plane::Daemon,
            SurfaceRequest::Approval => Plane::Approval,
        }
    }
}

// ─── prompt / completion types ──────────────────────────────────────────────

/// One conversation turn message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct Message {
    /// `"system"`, `"user"`, `"assistant"`, or `"tool"`.
    pub role: String,
    /// Free-text body.
    pub content: String,
}

impl Message {
    /// Build a message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// A prompt handed to a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Prompt {
    /// Model id (`anthropic/claude-3.5-sonnet`, …).
    pub model: String,
    /// Ordered conversation messages.
    pub messages: Vec<Message>,
    /// Optional `max_tokens` hint.
    pub max_tokens: Option<u32>,
    /// Optional sampling temperature.
    pub temperature: Option<f64>,
}

impl Prompt {
    /// Build a prompt.
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
        }
    }
}

/// Provider output for a single completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Completion {
    /// Generated text.
    pub text: String,
    /// Echo of the requested model id.
    pub model: String,
    /// One of `"stop"`, `"length"`, `"tool"`, `"content_filter"`.
    pub finish_reason: String,
    /// Token counters.
    pub usage: TokenCount,
    /// Optional structured tool calls. Providers that surface
    /// `tool_calls` natively (OpenAI, Anthropic, Gemini) populate this
    /// vector; the inline-markup fallback in
    /// [`agent_loop::parse_inline_tool_calls`] runs only when this is
    /// empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<agent_loop::ToolCall>,
}

/// Approximate token counters; populated to whatever fidelity the
/// underlying provider offers.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TokenCount {
    /// Tokens accepted as prompt input.
    pub prompt: u32,
    /// Tokens produced as completion output.
    pub completion: u32,
}

impl TokenCount {
    /// Build a fresh token count. Required because the struct is
    /// `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(prompt: u32, completion: u32) -> Self {
        Self { prompt, completion }
    }

    /// Sum of prompt + completion tokens (saturating).
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.prompt.saturating_add(self.completion)
    }
}

impl Completion {
    /// Build a fresh completion. Required because the struct is
    /// `#[non_exhaustive]`.
    #[must_use]
    pub fn new(
        text: impl Into<String>,
        model: impl Into<String>,
        finish_reason: impl Into<String>,
        usage: TokenCount,
    ) -> Self {
        Self {
            text: text.into(),
            model: model.into(),
            finish_reason: finish_reason.into(),
            usage,
            tool_calls: Vec::new(),
        }
    }

    /// Attach a list of structured tool calls. The loop driver
    /// prefers these over inline-markup parsing.
    #[must_use]
    pub fn with_tool_calls(mut self, calls: Vec<agent_loop::ToolCall>) -> Self {
        self.tool_calls = calls;
        self
    }
}

// ─── provider handle ────────────────────────────────────────────────────────

/// Error returned by a [`ProviderHandle`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// Network or transport failure talking to the upstream provider.
    #[error("provider transport: {0}")]
    Transport(String),
    /// Upstream returned a non-success response.
    #[error("provider returned error {code}: {message}")]
    Upstream {
        /// HTTP-style error code.
        code: u16,
        /// Free-text message from the provider.
        message: String,
    },
    /// The requested model id is not in the provider's catalogue.
    #[error("unknown model: {0}")]
    UnknownModel(String),
}

/// Result alias for [`ProviderHandle`].
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Async provider trait. Phase 4 lands the real vendor drivers
/// (`gaussclaw-providers::anthropic` etc.); Phase 1 only needs the trait
/// surface plus the [`EchoProvider`] for end-to-end testing.
#[async_trait]
pub trait ProviderHandle: Send + Sync {
    /// Provider id (`anthropic`, `openai`, `openrouter`, …).
    fn name(&self) -> &str;

    /// Run a prompt and return a single completion.
    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion>;
}

/// Deterministic echo provider used by tests and the Phase 1 demo binary.
///
/// Returns the last `user` message back wrapped in `(echo: ...)`. Real
/// vendor drivers replace this in Phase 4.
#[derive(Debug, Clone)]
pub struct EchoProvider {
    name: String,
}

impl EchoProvider {
    /// Build an echo provider with a chosen name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Default for EchoProvider {
    fn default() -> Self {
        Self::new("echo")
    }
}

#[async_trait]
impl ProviderHandle for EchoProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let last_user = prompt
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map_or("", |m| m.content.as_str())
            .to_string();
        let text = format!("(echo: {last_user})");
        let completion_tokens = u32::try_from(text.len() / 4).unwrap_or(u32::MAX);
        let prompt_tokens: u32 = prompt
            .messages
            .iter()
            .map(|m| u32::try_from(m.content.len() / 4).unwrap_or(u32::MAX))
            .sum();
        Ok(Completion {
            text,
            model: prompt.model.clone(),
            finish_reason: "stop".into(),
            usage: TokenCount {
                prompt: prompt_tokens,
                completion: completion_tokens,
            },
            tool_calls: Vec::new(),
        })
    }
}

// ─── turn policy ────────────────────────────────────────────────────────────

/// Error returned by [`TurnPolicy::run`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TurnError {
    /// The kernel refused the turn at admission.
    #[error("admit denied: {0:?}")]
    Denied(#[from] GaussError),
    /// The provider failed.
    #[error("provider: {0}")]
    Provider(#[from] ProviderError),
    /// Tool dispatch failed inside the HWCA worker (schema gate
    /// refusal, depth bound, or tool-side error). Carries the
    /// formatted underlying [`gauss_core::GaussError`] message.
    #[error("tool: {0}")]
    Tool(String),
    /// The prompt was malformed.
    #[error("invalid prompt: {0}")]
    Invalid(String),
}

/// Result alias for [`TurnPolicy::run`].
pub type TurnResult<T> = Result<T, TurnError>;

/// The Hermes `run_conversation` body, lifted into a Differential
/// Turn Engine-shaped lifecycle:
///
/// 1. **Admit.** Joint capability/taint check against the kernel.
/// 2. **Provider call.** Dispatch the prompt to the configured provider.
/// 3. **Return.** Hand the completion back to the caller.
///
/// Phase 2 inserts step 1.5 (WAL append) and step 3.5 (receipt sign).
/// Phase 3 inserts tool execution between provider call and return.
/// Phase 4 swaps [`EchoProvider`] for real vendor drivers. The trait
/// surface here is final.
pub struct TurnPolicy {
    kernel: KernelHandle,
    provider: Arc<dyn ProviderHandle>,
    /// Optional audit trace — every turn writes a start / complete entry
    /// to it. When `None`, audit recording is a no-op.
    audit: Option<AuditTrace>,
    /// Optional session store — every turn writes the user message AND
    /// the assistant completion as separate [`gaussclaw_store::Turn`]
    /// rows. When `None`, no persistence.
    store: Option<Arc<gaussclaw_store::SessionStore>>,
    /// Optional tool registry — when set, [`Self::dispatch_tool`]
    /// resolves a tool name into a [`gauss_traits::ToolTrait`] handle
    /// and runs it through the HWCA worker spawner.
    tools: Option<Arc<gaussclaw_tools::ToolRegistry>>,
    /// HWCA worker spawner — shared across every tool dispatch. Default
    /// is a fresh in-process spawner with no sandbox; production
    /// deployments swap to a Composite-Sandbox-attached spawner.
    spawner: Arc<gauss_hwca::WorkerSpawner>,
    /// Capability that every turn must satisfy. Default `NETWORK_GET`.
    required_cap: CapToken,
    /// Optional capability lower-bound contributed by a meta-router's
    /// catalogue — the **intersection** of every leaf's `cap_required`.
    ///
    /// When set, the admit gate must satisfy
    /// `required_cap ⊔ catalogue_lower_bound`. The intuition: the
    /// lower-bound is the minimum capability mask every leaf agrees on,
    /// so a grant that doesn't dominate it cannot reach **any** leaf in
    /// the catalogue. The kernel refuses the turn before the router is
    /// even consulted. Hermes has no equivalent gate.
    catalogue_lower_bound: Option<CapToken>,
}

impl TurnPolicy {
    /// Build a policy with a chosen provider and no audit trace.
    pub fn new(kernel: KernelHandle, provider: Arc<dyn ProviderHandle>) -> Self {
        Self {
            kernel,
            provider,
            audit: None,
            store: None,
            tools: None,
            spawner: Arc::new(gauss_hwca::WorkerSpawner::new()),
            required_cap: CapToken::NETWORK_GET,
            catalogue_lower_bound: None,
        }
    }

    /// Attach a [`gaussclaw_tools::ToolRegistry`]. Tool dispatch
    /// becomes available via [`Self::dispatch_tool`].
    #[must_use]
    pub fn with_tools(mut self, tools: Arc<gaussclaw_tools::ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Attach a caller-supplied [`gauss_hwca::WorkerSpawner`]. The
    /// default is an unsandboxed in-process spawner; production
    /// deployments build one with `gauss_sandbox::CompositeSandbox`
    /// attached.
    #[must_use]
    pub fn with_spawner(mut self, spawner: Arc<gauss_hwca::WorkerSpawner>) -> Self {
        self.spawner = spawner;
        self
    }

    /// Borrow the tool registry, if any.
    pub const fn tools(&self) -> Option<&Arc<gaussclaw_tools::ToolRegistry>> {
        self.tools.as_ref()
    }

    /// Dispatch a single tool call.
    ///
    /// Lifecycle (WAL-before-effect, Axiom A1):
    ///
    /// 1. Look up the tool in the registry. Unknown id → error.
    /// 2. **Audit `Inbound`** — recorded BEFORE admit, so even a
    ///    denied tool call leaves a chain-anchored record (this is
    ///    the contract; the user-message persist path in
    ///    `run_in_session` follows the same rule).
    /// 3. Kernel admit with the tool's declared `cap_required` and
    ///    the supplied `incoming_taint`. Refusal terminates dispatch.
    /// 4. [`gauss_hwca::WorkerSpawner::spawn_and_invoke`] — runs the
    ///    tool inside an HWCA worker; only the schema-validated
    ///    [`gauss_traits::ValidatedValue`] crosses back.
    ///
    /// # Errors
    /// - [`TurnError::Invalid`] when no registry is attached or the
    ///   tool id is unknown.
    /// - [`TurnError::Denied`] when the kernel refuses admit.
    /// - [`TurnError::Tool`] wrapping the HWCA / tool / schema-gate
    ///   error otherwise.
    pub async fn dispatch_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        incoming_taint: TaintLabel,
        depth: u32,
    ) -> TurnResult<gauss_traits::ValidatedValue> {
        let registry = self
            .tools
            .as_ref()
            .ok_or_else(|| TurnError::Invalid("no tool registry attached".into()))?;
        let tool = registry
            .get(tool_name)
            .ok_or_else(|| TurnError::Invalid(format!("unknown tool: {tool_name}")))?;
        // Audit the tool inbound BEFORE admit. Even a refused tool
        // call leaves a chain-anchored record (WAL-before-effect, A1).
        // The registry lookup precedes this only because resolving the
        // tool name to its manifest is the input-parse step — a wholly
        // unknown id has no inbound to record.
        if let Some(audit) = &self.audit {
            let body_bytes = serde_json::to_vec(&args).unwrap_or_default();
            audit
                .record_inbound(
                    format!("tool:{tool_name}"),
                    "agent",
                    &body_bytes,
                    incoming_taint,
                    self.kernel.plane_for(SurfaceRequest::SdkChat),
                )
                .await;
        }
        // Kernel admit: the tool's declared cap_required vs the
        // current grant, under the incoming taint floor.
        self.kernel
            .admit(tool.manifest().cap_required, incoming_taint)?;
        let validated = self
            .spawner
            .spawn_and_invoke(tool.as_ref(), args, incoming_taint, depth)
            .await
            .map_err(|e| TurnError::Tool(format!("{e:?}")))?;
        Ok(validated)
    }

    /// Attach an [`AuditTrace`]. Every turn now records `TurnStart`
    /// before admit and `TurnComplete` after the provider returns.
    #[must_use]
    pub fn with_audit(mut self, trace: AuditTrace) -> Self {
        self.audit = Some(trace);
        self
    }

    /// Attach a [`gaussclaw_store::SessionStore`]. Every turn now
    /// persists the user prompt and the assistant completion as
    /// chain-protected, lineage-linked rows.
    #[must_use]
    pub fn with_store(mut self, store: Arc<gaussclaw_store::SessionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Borrow the audit trace, if any.
    pub const fn audit(&self) -> Option<&AuditTrace> {
        self.audit.as_ref()
    }

    /// Borrow the session store, if any.
    pub const fn store(&self) -> Option<&Arc<gaussclaw_store::SessionStore>> {
        self.store.as_ref()
    }

    /// Override the required capability.
    #[must_use]
    pub const fn with_required_cap(mut self, cap: CapToken) -> Self {
        self.required_cap = cap;
        self
    }

    /// Attach a meta-router catalogue's capability lower-bound.
    ///
    /// The lower-bound is the intersection (bit-AND) of every leaf's
    /// `cap_required` in the catalogue. With this set, every turn's
    /// admit cap becomes `required_cap ⊔ catalogue_lower_bound`, so the
    /// kernel refuses a turn whose grant doesn't dominate the bound —
    /// i.e. a turn from which **no** leaf in the catalogue is
    /// reachable. The router never sees an admit it can't honour.
    ///
    /// Callers compute the bound from
    /// `gaussclaw_providers::Catalogue::capability_lower_bound` and pass
    /// it here, keeping the agent crate free of a back-dep on the
    /// provider plane.
    #[must_use]
    pub const fn with_catalogue_lower_bound(mut self, bound: CapToken) -> Self {
        self.catalogue_lower_bound = Some(bound);
        self
    }

    /// The capability the admit gate actually checks: the configured
    /// `required_cap` joined with the catalogue lower-bound (if any).
    #[must_use]
    pub fn effective_required_cap(&self) -> CapToken {
        self.catalogue_lower_bound
            .map_or(self.required_cap, |b| self.required_cap.join(b))
    }

    /// Borrow the kernel handle.
    pub const fn kernel(&self) -> &KernelHandle {
        &self.kernel
    }

    /// Provider name (for logging / receipt content).
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Run one turn.
    ///
    /// Lifecycle (WAL-before-effect, Axiom A1):
    ///
    /// 1. Validate prompt.
    /// 2. (Optional) Audit `TurnStart`.
    /// 3. (Optional) Persist user message → SessionStore. The store
    ///    write advances the chain-protected receipt log; on failure
    ///    the whole turn refuses.
    /// 4. Kernel admit.
    /// 5. Provider call.
    /// 6. (Optional) Persist assistant completion → SessionStore.
    /// 7. (Optional) Audit `TurnComplete`.
    ///
    /// The `session_id` is optional. When present, the user + assistant
    /// turns are appended to that session; when absent, the agent runs
    /// "headless" (e.g. one-shot CLI invocation) with only audit trace.
    pub async fn run(&self, prompt: Prompt, taint: TaintLabel) -> TurnResult<Completion> {
        self.run_in_session(prompt, taint, None).await
    }

    /// Same as [`Self::run`] but persists the turn into `session_id`'s
    /// stream. Returns the assistant completion plus the assigned turn
    /// id pair when a store is attached.
    pub async fn run_in_session(
        &self,
        prompt: Prompt,
        taint: TaintLabel,
        session_id: Option<&str>,
    ) -> TurnResult<Completion> {
        if prompt.messages.is_empty() {
            return Err(TurnError::Invalid("prompt has no messages".into()));
        }

        // WAL-before-effect: audit entry is recorded BEFORE admit, so
        // even a denied turn is auditable.
        if let Some(audit) = &self.audit {
            audit
                .record_turn_start(
                    session_id.unwrap_or(""),
                    &prompt.model,
                    self.provider.name(),
                    self.kernel.plane_for(SurfaceRequest::SdkChat),
                    taint,
                )
                .await;
        }

        // Persist the user message BEFORE admit. Identical to the audit
        // discipline: even a refused turn leaves a chain-anchored record.
        let mut parent_id: Option<u64> = None;
        if let (Some(store), Some(sid)) = (&self.store, session_id) {
            let last_user = prompt
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map_or("", |m| m.content.as_str());
            let (user_turn, _head) = store
                .append_turn(sid, None, "user", last_user, taint)
                .await
                .map_err(|e| TurnError::Invalid(format!("store: {e}")))?;
            parent_id = Some(user_turn.id);
        }

        self.kernel.admit(self.effective_required_cap(), taint)?;
        let completion = self.provider.complete(&prompt).await?;

        if let (Some(store), Some(sid)) = (&self.store, session_id) {
            let (_assistant_turn, _head) = store
                .append_turn(sid, parent_id, "assistant", &completion.text, taint)
                .await
                .map_err(|e| TurnError::Invalid(format!("store: {e}")))?;
        }

        if let Some(audit) = &self.audit {
            audit
                .record_turn_complete(
                    session_id.unwrap_or(""),
                    &completion.model,
                    self.provider.name(),
                    completion.usage.prompt,
                    completion.usage.completion,
                    &completion.finish_reason,
                )
                .await;
        }
        Ok(completion)
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{CapToken, TaintLabel};

    #[test]
    fn permissive_kernel_admits_a_trusted_request() {
        let h = KernelHandle::permissive();
        h.admit(CapToken::FILESYSTEM_READ, TaintLabel::Trusted)
            .expect("permissive kernel should admit a trusted read");
    }

    #[test]
    fn permissive_kernel_denies_post_under_web_taint() {
        let h = KernelHandle::permissive();
        let err = h
            .admit(CapToken::NETWORK_POST, TaintLabel::Web)
            .expect_err("default declass blocks POST under Web taint");
        // Underlying error is a `Denied` / `TaintTooHigh`; we don't care
        // which exact variant for this gate test — just that it errors.
        let _ = err;
    }

    #[test]
    fn plane_selector_maps_user_sync_to_conversation() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::UserSync), Plane::Conversation);
        assert_eq!(h.plane_for(SurfaceRequest::SdkChat), Plane::Conversation);
        assert_eq!(h.plane_for(SurfaceRequest::Channel), Plane::Conversation);
    }

    #[test]
    fn plane_selector_maps_scheduled_to_daemon() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::Scheduled), Plane::Daemon);
    }

    #[test]
    fn plane_selector_maps_approval_to_approval() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::Approval), Plane::Approval);
    }

    // ── TurnPolicy tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn echo_provider_returns_canned_completion() {
        let p = EchoProvider::default();
        let prompt = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![
                Message::new("system", "you are a test fixture"),
                Message::new("user", "hello"),
            ],
        );
        let out = p.complete(&prompt).await.expect("complete");
        assert_eq!(out.model, "anthropic/claude-3.5-sonnet");
        assert!(out.text.contains("hello"));
        assert_eq!(out.finish_reason, "stop");
    }

    #[tokio::test]
    async fn turn_policy_admits_and_dispatches() {
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        );
        let prompt = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![Message::new("user", "hi")],
        );
        let out = tp
            .run(prompt, TaintLabel::User)
            .await
            .expect("turn under permissive kernel");
        assert!(out.text.contains("hi"));
    }

    #[tokio::test]
    async fn turn_policy_refuses_empty_prompt() {
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        );
        let err = tp
            .run(Prompt::new("m", vec![]), TaintLabel::User)
            .await
            .unwrap_err();
        assert!(matches!(err, TurnError::Invalid(_)));
    }

    #[tokio::test]
    async fn turn_policy_denial_propagates() {
        use gauss_core::CapToken;
        use gauss_kernel::PrivilegedKernel;
        // Bottom grant blocks every cap → every admit fails.
        let empty = Arc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let tp = TurnPolicy::new(KernelHandle::new(empty), Arc::new(EchoProvider::default()));
        let prompt = Prompt::new("m", vec![Message::new("user", "hi")]);
        let err = tp.run(prompt, TaintLabel::User).await.unwrap_err();
        assert!(matches!(err, TurnError::Denied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn catalogue_lower_bound_augments_admit_cap() {
        use gauss_core::CapToken;
        use gauss_kernel::PrivilegedKernel;
        // Grant carries NETWORK_GET but NOT FILESYSTEM_READ. A catalogue
        // whose lower-bound requires FILESYSTEM_READ should make every
        // turn refuse, even though the bare required_cap is satisfied.
        let kernel = Arc::new(PrivilegedKernel::new(CapToken::NETWORK_GET));
        let tp = TurnPolicy::new(KernelHandle::new(kernel), Arc::new(EchoProvider::default()))
            .with_catalogue_lower_bound(CapToken::FILESYSTEM_READ);
        let prompt = Prompt::new("m", vec![Message::new("user", "hi")]);
        let err = tp.run(prompt, TaintLabel::User).await.unwrap_err();
        assert!(matches!(err, TurnError::Denied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn catalogue_lower_bound_satisfied_admits_turn() {
        use gauss_core::CapToken;
        use gauss_kernel::PrivilegedKernel;
        // Grant covers both NETWORK_GET and FILESYSTEM_READ → admit
        // succeeds, completion returns normally.
        let grant = CapToken::NETWORK_GET.join(CapToken::FILESYSTEM_READ);
        let kernel = Arc::new(PrivilegedKernel::new(grant));
        let tp = TurnPolicy::new(KernelHandle::new(kernel), Arc::new(EchoProvider::default()))
            .with_catalogue_lower_bound(CapToken::FILESYSTEM_READ);
        let prompt = Prompt::new("m", vec![Message::new("user", "hi")]);
        let out = tp.run(prompt, TaintLabel::User).await.expect("admit");
        assert!(out.text.contains("hi"));
    }

    #[test]
    fn effective_required_cap_joins_bound() {
        use gauss_core::CapToken;
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_required_cap(CapToken::NETWORK_GET)
        .with_catalogue_lower_bound(CapToken::FILESYSTEM_READ);
        let eff = tp.effective_required_cap();
        assert!(eff.contains(CapToken::NETWORK_GET));
        assert!(eff.contains(CapToken::FILESYSTEM_READ));
    }

    #[test]
    fn effective_required_cap_without_bound_is_required_cap() {
        use gauss_core::CapToken;
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_required_cap(CapToken::NETWORK_GET);
        assert_eq!(tp.effective_required_cap(), CapToken::NETWORK_GET);
    }

    #[test]
    fn token_count_total_saturates() {
        let t = TokenCount {
            prompt: u32::MAX,
            completion: 5,
        };
        assert_eq!(t.total(), u32::MAX);
    }

    // ── Store integration ───────────────────────────────────────────────────

    #[tokio::test]
    async fn run_in_session_persists_user_and_assistant_turns() {
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let sess = store.create_session("test", "echo").await;
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_store(store.clone());

        let prompt = Prompt::new("echo", vec![Message::new("user", "marker-789")]);
        let _ = tp
            .run_in_session(prompt, TaintLabel::User, Some(&sess.id))
            .await
            .expect("turn");

        let turns = store.list_session_turns(&sess.id).await;
        assert_eq!(turns.len(), 2, "user + assistant must both persist");
        assert_eq!(turns[0].role, "user");
        assert!(turns[0].content.contains("marker-789"));
        assert_eq!(turns[1].role, "assistant");
        assert!(turns[1].content.contains("marker-789"));
        assert_eq!(turns[1].parent_id, Some(turns[0].id));
    }

    #[tokio::test]
    async fn run_in_session_chains_lineage_to_parent() {
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let sess = store.create_session("test", "echo").await;
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_store(store.clone());

        let prompt = Prompt::new("echo", vec![Message::new("user", "first")]);
        let _ = tp
            .run_in_session(prompt, TaintLabel::User, Some(&sess.id))
            .await
            .unwrap();
        // Assistant turn's lineage edge must be signed.
        let turns = store.list_session_turns(&sess.id).await;
        let edge = store.lineage_edge(turns[1].id).await.unwrap();
        assert_eq!(edge.from, turns[0].id);
        assert_eq!(edge.to, turns[1].id);
        assert_eq!(edge.commit_hex.len(), 64);
    }

    // ── tool dispatch (Phase 3) ─────────────────────────────────────────────

    #[tokio::test]
    async fn dispatch_tool_without_registry_is_invalid() {
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        );
        let err = tp
            .dispatch_tool("echo", serde_json::json!({}), TaintLabel::User, 0)
            .await
            .unwrap_err();
        assert!(matches!(err, TurnError::Invalid(_)));
    }

    #[tokio::test]
    async fn dispatch_tool_unknown_id_is_invalid() {
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_tools(Arc::new(gaussclaw_tools::default_registry()));
        let err = tp
            .dispatch_tool("nope", serde_json::json!({}), TaintLabel::User, 0)
            .await
            .unwrap_err();
        assert!(matches!(err, TurnError::Invalid(_)));
    }

    #[tokio::test]
    async fn dispatch_tool_echo_returns_validated_value() {
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_tools(Arc::new(gaussclaw_tools::default_registry()));
        let out = tp
            .dispatch_tool(
                "echo",
                serde_json::json!({ "text": "marker-xyz" }),
                TaintLabel::User,
                0,
            )
            .await
            .expect("echo dispatch");
        assert_eq!(out.value["echo"], "marker-xyz");
        // Output taint joins incoming with the tool's declared default
        // (Web for the HWCA default). Result is one of those two.
        assert!(matches!(out.taint, TaintLabel::Web | TaintLabel::User));
    }

    #[tokio::test]
    async fn dispatch_tool_admit_denial_propagates() {
        use gauss_core::CapToken;
        use gauss_kernel::PrivilegedKernel;
        // Empty grant: file_read requires FILESYSTEM_READ → admit fails.
        let empty = Arc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let tp = TurnPolicy::new(KernelHandle::new(empty), Arc::new(EchoProvider::default()))
            .with_tools(Arc::new(gaussclaw_tools::default_registry()));
        let err = tp
            .dispatch_tool(
                "file_read",
                serde_json::json!({ "path": "/tmp/x" }),
                TaintLabel::User,
                0,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TurnError::Denied(_)), "got {err:?}");
    }

    /// WAL-before-effect (Axiom A1) for the tool-dispatch path.
    ///
    /// A denied tool call MUST still leave a chain-anchored audit
    /// record. Phase 3 review caught an ordering bug (record happened
    /// after admit); this test locks the fix in place.
    #[tokio::test]
    async fn dispatch_tool_audits_before_admit_so_denials_are_logged() {
        use gauss_core::CapToken;
        use gauss_kernel::PrivilegedKernel;
        let empty = Arc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let audit = AuditTrace::new();
        let head_before = audit.head().await;
        let tp = TurnPolicy::new(KernelHandle::new(empty), Arc::new(EchoProvider::default()))
            .with_audit(audit.clone())
            .with_tools(Arc::new(gaussclaw_tools::default_registry()));

        let _err = tp
            .dispatch_tool(
                "file_read",
                serde_json::json!({ "path": "/tmp/x" }),
                TaintLabel::User,
                0,
            )
            .await
            .expect_err("BOTTOM-grant kernel must deny file_read");

        // Even though admit denied, the audit chain must advance
        // — the inbound record was written BEFORE the admit check.
        let head_after = audit.head().await;
        assert_ne!(
            head_before.as_bytes(),
            head_after.as_bytes(),
            "denied tool call must still advance the audit chain"
        );
    }

    #[tokio::test]
    async fn run_in_session_does_not_persist_without_session_id() {
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let tp = TurnPolicy::new(
            KernelHandle::permissive(),
            Arc::new(EchoProvider::default()),
        )
        .with_store(store.clone());

        let prompt = Prompt::new("echo", vec![Message::new("user", "headless")]);
        let _ = tp.run(prompt, TaintLabel::User).await.unwrap();

        // No session was provided → no rows persisted.
        let head = store.chain_head().await.unwrap();
        assert_eq!(head.length, 0);
    }
}
