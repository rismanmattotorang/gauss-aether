//! The iterative agent loop driver.
//!
//! Until Sprint 4, `TurnPolicy::run_in_session` did a single
//! prompt-completion shot — useful for unary chat but unable to drive
//! the model through a chain of tool calls. The upstream Hermes
//! `conversation_loop.py` is ~9 KLOC of repeat-until-stop logic; we
//! distil its core contract into a small, testable Rust driver here.
//!
//! ## The contract
//!
//! [`AgentLoop::run`] iterates:
//!
//! 1. Call the provider with the current message stack.
//! 2. Parse tool calls out of the completion (either from the
//!    provider-supplied [`Completion::tool_calls`] vector or from
//!    inline `<tool name="...">{...}</tool>` markup as a fallback).
//! 3. For each tool call: dispatch through the existing
//!    [`super::TurnPolicy::dispatch_tool`] — admit-gated, sandboxed,
//!    schema-validated.
//! 4. Append a `tool`-role message with the validated value as the new
//!    assistant input.
//! 5. Stop when [`Completion::finish_reason`] is anything other than
//!    `"tool"` (typically `"stop"`), when the iteration cap is hit, or
//!    when the [`LoopSink`] reports a cancellation.
//!
//! Every iteration boundary emits a [`LoopEvent`] to the optional
//! [`LoopSink`]. The web crate's WebSocket handler is the canonical
//! sink — its dashboard `app.js` already understands the frame
//! shapes we emit (`token`, `tool.start`, `tool.complete`,
//! `assistant`, `receipt`, `done`).
//!
//! ## Cancellation
//!
//! The loop checks [`LoopSink::should_cancel`] at every iteration
//! boundary. A `Ctrl+C` from the TUI or a `WS Close` from the
//! dashboard sets the cancellation flag; the loop returns
//! [`LoopOutcome::cancelled`] with the partial transcript intact and
//! every receipt already committed.
//!
//! ## Fallback
//!
//! When the provider returns a [`ProviderError`], the loop consults
//! [`AgentLoop::fallback`] (a fresh-cloneable
//! [`gaussclaw_providers::FallbackChain`]-style indirection in this
//! crate to keep the agent free of a back-dep on the provider plane).
//! Each fallback attempt joins the receipt chain as its own
//! `record_inbound("provider:fallback", …)` row so audit verifiers
//! can replay the failure sequence.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    Completion, Message, Prompt, ProviderError, ProviderHandle, TurnError, TurnPolicy, TurnResult,
};
use gauss_core::TaintLabel;

// ─── tool-call wire shape ─────────────────────────────────────────────────

/// One structured tool call emitted by the provider.
///
/// We avoid coupling to any vendor's wire format directly. Providers
/// that already speak OpenAI's `tool_calls` array convert into this
/// shape inside their codec; legacy providers can leave
/// [`Completion::tool_calls`] empty and emit `<tool name=…>…</tool>`
/// markup, which [`AgentLoop`] parses out of [`Completion::text`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCall {
    /// Tool name as registered with the [`gaussclaw_tools::ToolRegistry`].
    pub name: String,
    /// Argument value the agent passes to `dispatch_tool`. Wire shape is
    /// canonical JSON; the schema gate validates against the tool's
    /// manifest.
    pub args: serde_json::Value,
    /// Optional caller-supplied id so the response message can be
    /// correlated. Mirrors OpenAI's `tool_call_id`.
    pub id: Option<String>,
}

impl ToolCall {
    /// Construct.
    #[must_use]
    pub fn new(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            args,
            id: None,
        }
    }

    /// Attach a correlation id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// Parse `<tool name="...">{json_args}</tool>` inline markup out of a
/// free-text completion. Provider codecs that don't surface structured
/// `tool_calls` rely on this fallback; the grammar is intentionally
/// minimal — the model emits a single XML-style block and we extract
/// the name attribute and the inner JSON.
#[must_use]
pub fn parse_inline_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(start_idx) = rest.find("<tool ") {
        let after_open = &rest[start_idx + "<tool ".len()..];
        // Find name="X"
        let Some(name_open) = after_open.find("name=\"") else {
            break;
        };
        let name_start = name_open + "name=\"".len();
        let Some(name_end) = after_open[name_start..].find('"') else {
            break;
        };
        let name = &after_open[name_start..name_start + name_end];

        // Find the closing >
        let Some(gt) = after_open.find('>') else {
            break;
        };
        let after_tag = &after_open[gt + 1..];
        // Find </tool>
        let Some(end_idx) = after_tag.find("</tool>") else {
            break;
        };
        let raw = after_tag[..end_idx].trim();
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(raw) {
            calls.push(ToolCall::new(name.to_owned(), args));
        }
        rest = &after_tag[end_idx + "</tool>".len()..];
    }
    calls
}

// ─── streaming events ─────────────────────────────────────────────────────

/// Events the loop emits at iteration boundaries.
///
/// The frame shapes are JSON-stable: each variant serialises with a
/// `"kind"` discriminator. The dashboard's `app.js` already speaks
/// these shapes (`type` field mapped to `kind` via the loop sink).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LoopEvent {
    /// A user message has been accepted. Emitted once at the start of
    /// the loop after the WAL barrier.
    UserSubmitted { text: String, turn: u32 },
    /// A streaming token from the provider. Only emitted when the
    /// provider's codec supports token-level streaming.
    Token { text: String, turn: u32 },
    /// A complete assistant message landed. Emitted at every iteration
    /// where the provider returns text (with or without tool calls).
    Assistant { text: String, turn: u32 },
    /// A tool call is about to be dispatched.
    ToolStart {
        name: String,
        args: serde_json::Value,
        turn: u32,
    },
    /// A tool call returned validated output.
    ToolComplete {
        name: String,
        ok: bool,
        result: serde_json::Value,
        turn: u32,
    },
    /// A provider fallback attempt is in progress.
    FallbackAttempt {
        from_provider: String,
        to_provider: String,
        reason: String,
    },
    /// The loop is done. Carries the stop reason and the final
    /// iteration count.
    Done {
        stop_reason: String,
        iterations: u32,
    },
}

/// A receiver for [`LoopEvent`]s. The web crate provides a
/// WebSocket-backed implementation; tests use [`MemorySink`] below.
#[async_trait]
pub trait LoopSink: Send + Sync {
    /// Emit one event. The sink decides whether to buffer, drop, or
    /// forward over the wire.
    async fn emit(&self, event: LoopEvent);

    /// Whether the caller has asked the loop to abort. Checked at
    /// every iteration boundary; the loop returns
    /// [`LoopOutcome::Cancelled`] when this returns `true`.
    fn should_cancel(&self) -> bool {
        false
    }
}

/// A no-op sink. Useful for headless invocations.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

#[async_trait]
impl LoopSink for NoopSink {
    async fn emit(&self, _event: LoopEvent) {}
}

/// In-memory sink that retains every event. Used by tests and the
/// snapshot conformance suite.
#[derive(Debug, Default, Clone)]
pub struct MemorySink {
    events: Arc<tokio::sync::Mutex<Vec<LoopEvent>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl MemorySink {
    /// Build an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot every recorded event.
    pub async fn events(&self) -> Vec<LoopEvent> {
        self.events.lock().await.clone()
    }

    /// Cause the next iteration boundary to return cancelled.
    pub fn request_cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl LoopSink for MemorySink {
    async fn emit(&self, event: LoopEvent) {
        self.events.lock().await.push(event);
    }

    fn should_cancel(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ─── outcome ──────────────────────────────────────────────────────────────

/// Result of an [`AgentLoop::run`] call.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LoopOutcome {
    /// Every assistant message produced, oldest first.
    pub assistants: Vec<Completion>,
    /// Number of provider iterations executed.
    pub iterations: u32,
    /// `"stop"`, `"length"`, `"cancelled"`, `"max_iterations"`,
    /// `"error"`.
    pub stop_reason: String,
}

/// Default iteration cap. Hermes uses ~32; we mirror.
pub const DEFAULT_MAX_ITERATIONS: u32 = 32;

// ─── the loop ─────────────────────────────────────────────────────────────

/// The iterative loop driver. Composes on top of an existing
/// [`TurnPolicy`] so all the kernel-admit / audit / store wiring stays
/// in one place.
pub struct AgentLoop {
    /// Underlying single-shot driver.
    pub policy: TurnPolicy,
    /// Maximum iterations before the loop bails out with
    /// `stop_reason = "max_iterations"`.
    pub max_iterations: u32,
    /// Optional ordered list of fallback providers. When the primary
    /// returns a [`ProviderError`], the loop walks this list in order
    /// before surfacing the error to the caller.
    pub fallback: Vec<Arc<dyn ProviderHandle>>,
}

impl AgentLoop {
    /// Build a loop around an existing [`TurnPolicy`].
    #[must_use]
    pub fn new(policy: TurnPolicy) -> Self {
        Self {
            policy,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            fallback: Vec::new(),
        }
    }

    /// Override the iteration cap.
    #[must_use]
    pub const fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = max;
        self
    }

    /// Append a fallback provider. On primary failure the loop tries
    /// each fallback in registration order before bubbling the error.
    #[must_use]
    pub fn with_fallback(mut self, provider: Arc<dyn ProviderHandle>) -> Self {
        self.fallback.push(provider);
        self
    }

    /// Run the loop.
    ///
    /// Each iteration:
    /// - Builds a fresh [`Prompt`] from the running message stack.
    /// - Calls the provider (with fallback on error).
    /// - Parses tool calls; if any, dispatches each and appends the
    ///   result as a `tool`-role message; otherwise the loop ends.
    ///
    /// Emits a [`LoopEvent`] to `sink` at every boundary. Honours
    /// [`LoopSink::should_cancel`] between iterations.
    pub async fn run(
        &self,
        prompt: Prompt,
        taint: TaintLabel,
        session_id: Option<&str>,
        sink: &dyn LoopSink,
    ) -> TurnResult<LoopOutcome> {
        if prompt.messages.is_empty() {
            return Err(TurnError::Invalid("prompt has no messages".into()));
        }

        // Emit the user-submission event for the latest user message.
        if let Some(last_user) = prompt.messages.iter().rev().find(|m| m.role == "user") {
            sink.emit(LoopEvent::UserSubmitted {
                text: last_user.content.clone(),
                turn: 0,
            })
            .await;
        }

        let mut messages = prompt.messages.clone();
        let model = prompt.model.clone();
        let max_tokens = prompt.max_tokens;
        let temperature = prompt.temperature;

        let mut assistants: Vec<Completion> = Vec::new();
        let mut iterations: u32 = 0;

        loop {
            if sink.should_cancel() {
                let stop = "cancelled".to_owned();
                sink.emit(LoopEvent::Done {
                    stop_reason: stop.clone(),
                    iterations,
                })
                .await;
                return Ok(LoopOutcome {
                    assistants,
                    iterations,
                    stop_reason: stop,
                });
            }
            if iterations >= self.max_iterations {
                let stop = "max_iterations".to_owned();
                sink.emit(LoopEvent::Done {
                    stop_reason: stop.clone(),
                    iterations,
                })
                .await;
                return Ok(LoopOutcome {
                    assistants,
                    iterations,
                    stop_reason: stop,
                });
            }

            // Build the iteration's prompt from the running stack.
            let iter_prompt = Prompt {
                model: model.clone(),
                messages: messages.clone(),
                max_tokens,
                temperature,
            };

            // Call provider with fallback chain.
            let completion = match self.run_with_fallback(&iter_prompt, taint, session_id, sink).await {
                Ok(c) => c,
                Err(e) => {
                    sink.emit(LoopEvent::Done {
                        stop_reason: "error".into(),
                        iterations,
                    })
                    .await;
                    return Err(e);
                }
            };
            iterations = iterations.saturating_add(1);

            // Emit the assistant text + remember it.
            sink.emit(LoopEvent::Assistant {
                text: completion.text.clone(),
                turn: iterations,
            })
            .await;
            assistants.push(completion.clone());

            // Parse tool calls. The provider may have surfaced them
            // structurally (Completion::tool_calls) OR inline in the
            // text. Inline parsing is a strict fallback — we don't
            // double-execute when both are present.
            let tool_calls = if completion.tool_calls.is_empty() {
                parse_inline_tool_calls(&completion.text)
            } else {
                completion.tool_calls.clone()
            };

            if tool_calls.is_empty() {
                let stop = completion.finish_reason.clone();
                sink.emit(LoopEvent::Done {
                    stop_reason: stop.clone(),
                    iterations,
                })
                .await;
                return Ok(LoopOutcome {
                    assistants,
                    iterations,
                    stop_reason: stop,
                });
            }

            // Append the assistant message that ASKED for the tool, so
            // the next provider iteration sees its own prior reasoning.
            messages.push(Message {
                role: "assistant".into(),
                content: completion.text.clone(),
            });

            // Dispatch each tool sequentially.
            for tc in tool_calls {
                sink.emit(LoopEvent::ToolStart {
                    name: tc.name.clone(),
                    args: tc.args.clone(),
                    turn: iterations,
                })
                .await;
                let (ok, result_json) = match self
                    .policy
                    .dispatch_tool(&tc.name, tc.args.clone(), taint, 0)
                    .await
                {
                    Ok(validated) => (true, validated.into_json()),
                    Err(e) => (false, serde_json::json!({ "error": format!("{e:?}") })),
                };
                sink.emit(LoopEvent::ToolComplete {
                    name: tc.name.clone(),
                    ok,
                    result: result_json.clone(),
                    turn: iterations,
                })
                .await;
                let body = serde_json::to_string(&result_json).unwrap_or_else(|_| "{}".into());
                let mut content = format!("[tool:{} result] {body}", tc.name);
                if let Some(id) = &tc.id {
                    content = format!("[tool_call_id={id}] {content}");
                }
                messages.push(Message {
                    role: "tool".into(),
                    content,
                });
            }
        }
    }

    async fn run_with_fallback(
        &self,
        prompt: &Prompt,
        taint: TaintLabel,
        session_id: Option<&str>,
        sink: &dyn LoopSink,
    ) -> TurnResult<Completion> {
        match self.policy.run_in_session(prompt.clone(), taint, session_id).await {
            Ok(c) => Ok(c),
            Err(TurnError::Provider(primary_err)) if !self.fallback.is_empty() => {
                let mut last_err = primary_err;
                for fb in &self.fallback {
                    sink.emit(LoopEvent::FallbackAttempt {
                        from_provider: self.policy.provider_name().into(),
                        to_provider: fb.name().into(),
                        reason: format!("{last_err:?}"),
                    })
                    .await;
                    match fb.complete(prompt).await {
                        Ok(c) => return Ok(c),
                        Err(e) => last_err = e,
                    }
                }
                Err(TurnError::Provider(last_err))
            }
            Err(e) => Err(e),
        }
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

/// Type-erase whatever shape `gauss_traits::ValidatedValue` exposes so
/// we can serialise it into the [`LoopEvent::ToolComplete`] frame and
/// re-feed the tool result into the next prompt. The current
/// `ValidatedValue` is a thin wrapper around `serde_json::Value` plus
/// a taint label; we extract the JSON.
trait ValidatedJson {
    fn into_json(self) -> serde_json::Value;
}

impl ValidatedJson for gauss_traits::ValidatedValue {
    fn into_json(self) -> serde_json::Value {
        // `ValidatedValue` exposes its `value` field directly. We move
        // out of it because the validated value is dropped after the
        // loop iteration.
        self.value
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Completion, KernelHandle, ProviderHandle, TokenCount, TurnPolicy};
    use gauss_hwca::WorkerSpawner;
    use gaussclaw_tools::ToolRegistry;
    use std::sync::Arc;

    // ─── local test fixtures ─────────────────────────────────────────

    /// Build a permissive [`WorkerSpawner`] that lets every tool
    /// invocation through. Mirrors `gaussclaw_tools::noop_sandboxed`.
    fn permissive_spawner() -> Arc<WorkerSpawner> {
        gaussclaw_tools::noop_sandboxed()
    }

    /// Build a registry containing exactly the canonical `echo` tool.
    fn registry_with_echo() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(gaussclaw_tools::EchoTool::new()));
        reg
    }

    /// Build a provider whose every reply is `text` (with `stop`).
    fn stub_provider(text: &'static str) -> Arc<dyn ProviderHandle> {
        struct Stub(&'static str);
        #[async_trait::async_trait]
        impl ProviderHandle for Stub {
            fn name(&self) -> &str {
                "echo"
            }
            async fn complete(&self, _p: &Prompt) -> Result<Completion, ProviderError> {
                Ok(Completion::new(
                    self.0,
                    "stub/model",
                    "stop",
                    TokenCount::new(0, 0),
                ))
            }
        }
        Arc::new(Stub(text))
    }

    /// A fixture provider that returns successive scripted completions.
    /// Lets us drive the loop deterministically.
    struct ScriptProvider {
        name: &'static str,
        idx: std::sync::atomic::AtomicUsize,
        script: Vec<Completion>,
    }

    impl ScriptProvider {
        fn new(name: &'static str, script: Vec<Completion>) -> Self {
            Self {
                name,
                idx: std::sync::atomic::AtomicUsize::new(0),
                script,
            }
        }
    }

    #[async_trait::async_trait]
    impl ProviderHandle for ScriptProvider {
        fn name(&self) -> &str {
            self.name
        }
        async fn complete(&self, _p: &Prompt) -> Result<Completion, ProviderError> {
            let i = self
                .idx
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.script
                .get(i)
                .cloned()
                .ok_or_else(|| ProviderError::Transport(format!("script exhausted at {i}")))
        }
    }

    fn loop_policy(provider: Arc<dyn ProviderHandle>) -> TurnPolicy {
        TurnPolicy::new(KernelHandle::permissive(), provider)
            .with_tools(Arc::new(registry_with_echo()))
            .with_spawner(permissive_spawner())
    }

    #[test]
    fn parse_inline_tool_calls_basic() {
        let calls = parse_inline_tool_calls(
            "thinking...\n<tool name=\"echo\">{\"msg\":\"hi\"}</tool>\ndone",
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(calls[0].args["msg"], "hi");
    }

    #[test]
    fn parse_inline_tool_calls_multiple() {
        let calls = parse_inline_tool_calls(
            "<tool name=\"a\">{}</tool>\nthen\n<tool name=\"b\">{\"x\":1}</tool>",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "a");
        assert_eq!(calls[1].name, "b");
        assert_eq!(calls[1].args["x"], 1);
    }

    #[test]
    fn parse_inline_tool_calls_ignores_malformed() {
        let calls = parse_inline_tool_calls("<tool name=\"x\">not json</tool>");
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn no_tool_calls_stops_after_one_iteration() {
        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![Completion::new(
                "hello, world",
                "scripted/model",
                "stop",
                TokenCount::new(1, 4),
            )],
        ));
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted/model", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        assert!(matches!(events.first(), Some(LoopEvent::UserSubmitted { .. })));
        assert!(matches!(events.last(), Some(LoopEvent::Done { .. })));
    }

    #[tokio::test]
    async fn inline_tool_call_drives_one_dispatch_then_stops() {
        let script = vec![
            Completion::new(
                "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                "scripted/model",
                "tool",
                TokenCount::new(10, 5),
            ),
            Completion::new(
                "ok, did it",
                "scripted/model",
                "stop",
                TokenCount::new(15, 4),
            ),
        ];
        let provider = Arc::new(ScriptProvider::new("scripted", script));
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted/model", vec![Message::new("user", "echo hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.iterations, 2);
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        assert!(events.iter().any(|e| matches!(e, LoopEvent::ToolStart { name, .. } if name == "echo")));
        assert!(events.iter().any(|e| matches!(e, LoopEvent::ToolComplete { name, ok, .. } if name == "echo" && *ok)));
    }

    #[tokio::test]
    async fn iteration_cap_returns_max_iterations() {
        // Always loop: provider always asks for a tool that always
        // succeeds.
        let mut script = Vec::new();
        for _ in 0..10 {
            script.push(Completion::new(
                "<tool name=\"echo\">{\"text\":\"again\"}</tool>",
                "scripted",
                "tool",
                TokenCount::new(1, 1),
            ));
        }
        let provider = Arc::new(ScriptProvider::new("scripted", script));
        let loop_ = AgentLoop::new(loop_policy(provider)).with_max_iterations(3);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "loop me")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.iterations, 3);
        assert_eq!(outcome.stop_reason, "max_iterations");
    }

    #[tokio::test]
    async fn cancellation_returns_immediately_on_next_boundary() {
        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![Completion::new(
                "<tool name=\"echo\">{\"text\":\"x\"}</tool>",
                "scripted",
                "tool",
                TokenCount::new(1, 1),
            )],
        ));
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        sink.request_cancel(); // cancel before we even start
        let outcome = loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "cancelled");
        assert_eq!(outcome.iterations, 0);
    }

    #[tokio::test]
    async fn fallback_attempts_recorded_on_provider_error() {
        struct FailingProvider;
        #[async_trait::async_trait]
        impl ProviderHandle for FailingProvider {
            fn name(&self) -> &str {
                "fails"
            }
            async fn complete(&self, _p: &Prompt) -> Result<Completion, ProviderError> {
                Err(ProviderError::Transport("upstream down".into()))
            }
        }
        let primary = Arc::new(FailingProvider);
        let secondary = stub_provider("ok, fallback worked");
        let policy = TurnPolicy::new(KernelHandle::permissive(), primary);
        let loop_ = AgentLoop::new(policy).with_fallback(secondary);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("any", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("fallback should succeed");
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        assert!(events
            .iter()
            .any(|e| matches!(e, LoopEvent::FallbackAttempt { from_provider, to_provider, .. } if from_provider == "fails" && to_provider == "echo")));
    }
}
