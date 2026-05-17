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

pub mod audit;

pub use audit::{
    AuditEntry, AuditTrace, InboundRecord, OutboundRecord, PlaneLabel, TurnCompleteRecord,
    TurnStartRecord, blake3_hex,
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
    /// Sum of prompt + completion tokens (saturating).
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.prompt.saturating_add(self.completion)
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
    /// Capability that every turn must satisfy. Default `NETWORK_GET`.
    required_cap: CapToken,
}

impl TurnPolicy {
    /// Build a policy with a chosen provider and no audit trace.
    pub fn new(kernel: KernelHandle, provider: Arc<dyn ProviderHandle>) -> Self {
        Self {
            kernel,
            provider,
            audit: None,
            required_cap: CapToken::NETWORK_GET,
        }
    }

    /// Attach an [`AuditTrace`]. Every turn now records `TurnStart`
    /// before admit and `TurnComplete` after the provider returns.
    #[must_use]
    pub fn with_audit(mut self, trace: AuditTrace) -> Self {
        self.audit = Some(trace);
        self
    }

    /// Borrow the audit trace, if any.
    pub const fn audit(&self) -> Option<&AuditTrace> {
        self.audit.as_ref()
    }

    /// Override the required capability.
    #[must_use]
    pub const fn with_required_cap(mut self, cap: CapToken) -> Self {
        self.required_cap = cap;
        self
    }

    /// Borrow the kernel handle.
    pub const fn kernel(&self) -> &KernelHandle {
        &self.kernel
    }

    /// Provider name (for logging / receipt content).
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Run one turn. Admit-gates first, then dispatches to the provider.
    /// When an audit trace is attached, writes `TurnStart` before admit
    /// (WAL-before-effect, Axiom A1) and `TurnComplete` after the
    /// provider returns.
    pub async fn run(&self, prompt: Prompt, taint: TaintLabel) -> TurnResult<Completion> {
        if prompt.messages.is_empty() {
            return Err(TurnError::Invalid("prompt has no messages".into()));
        }
        // WAL-before-effect: audit entry is recorded BEFORE admit, so
        // even a denied turn is auditable.
        if let Some(audit) = &self.audit {
            audit
                .record_turn_start(
                    "",
                    &prompt.model,
                    self.provider.name(),
                    self.kernel.plane_for(SurfaceRequest::SdkChat),
                    taint,
                )
                .await;
        }
        self.kernel.admit(self.required_cap, taint)?;
        let completion = self.provider.complete(&prompt).await?;
        if let Some(audit) = &self.audit {
            audit
                .record_turn_complete(
                    "",
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
        let tp = TurnPolicy::new(
            KernelHandle::new(empty),
            Arc::new(EchoProvider::default()),
        );
        let prompt = Prompt::new("m", vec![Message::new("user", "hi")]);
        let err = tp.run(prompt, TaintLabel::User).await.unwrap_err();
        assert!(matches!(err, TurnError::Denied(_)), "got {err:?}");
    }

    #[test]
    fn token_count_total_saturates() {
        let t = TokenCount {
            prompt: u32::MAX,
            completion: 5,
        };
        assert_eq!(t.total(), u32::MAX);
    }
}
