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

// Sprint-4 file; many wire-shape struct fields are self-documenting
// (`role`, `name`, `args`, etc.). Re-enable per-field docs once the
// final wire shape stabilises in Sprint 5+.
#![allow(
    missing_docs,
    unused_imports,
    clippy::too_long_first_doc_paragraph,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines,
    clippy::needless_lifetimes,
    clippy::needless_pass_by_value,
    clippy::elidable_lifetime_names,
    rustdoc::broken_intra_doc_links
)]

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    audit::AuditTrace,
    compaction::{CompactionRecord, Compactor},
    Completion, Message, Prompt, ProviderError, ProviderHandle, TurnError, TurnPolicy, TurnResult,
};
use gauss_core::TaintLabel;
use gauss_hooks::{HookBus, PostToolEvent, PreToolEvent};

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
    /// A `PreToolHook` denied a tool call before dispatch. The loop
    /// records the reason and falls through with a synthetic
    /// `"tool"`-role error message so the model sees the denial.
    /// (OpenHarness-inspired lifecycle hook surface.)
    ToolDenied {
        name: String,
        reason: String,
        turn: u32,
    },
    /// A `PreToolHook` emitted a non-blocking warning. The loop still
    /// dispatches the tool; the message is surfaced for observability.
    ToolWarn {
        name: String,
        message: String,
        turn: u32,
    },
    /// A provider fallback attempt is in progress.
    FallbackAttempt {
        from_provider: String,
        to_provider: String,
        reason: String,
    },
    /// Auto-Compaction fired between iterations. The loop summarised
    /// older messages and continues running. (OpenHarness-inspired
    /// Auto-Compaction surface.)
    Compacted {
        collapsed: usize,
        retained: usize,
        before_chars: usize,
        after_chars: usize,
        turn: u32,
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

/// Share-able cancel-flag handle.
///
/// Sprint 9 §2 — every front-end runtime (TUI's `Ctrl+C`, the
/// dashboard's `WS Close`, the ACP server's `cancel` RPC) holds an
/// independent `CancelHandle` that flips the same atomic flag the
/// `AgentLoop` polls between iterations. The handle is intentionally
/// minimal — it doesn't carry the event stream, so a front-end that
/// only needs to cancel doesn't accept a full [`MemorySink`].
///
/// Cheap to clone (`Arc<AtomicBool>`).
#[derive(Debug, Default, Clone)]
pub struct CancelHandle {
    inner: Arc<std::sync::atomic::AtomicBool>,
}

impl CancelHandle {
    /// Build a fresh, un-fired handle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the cancel flag. The next [`AgentLoop`] iteration boundary
    /// will return [`LoopOutcome::stop_reason`] = `"cancelled"`.
    pub fn request_cancel(&self) {
        self.inner.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Inspect the cancel flag.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// In-memory sink that retains every event. Used by tests and the
/// snapshot conformance suite.
#[derive(Debug, Default, Clone)]
pub struct MemorySink {
    events: Arc<tokio::sync::Mutex<Vec<LoopEvent>>>,
    cancel: CancelHandle,
}

impl MemorySink {
    /// Build an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a sink that shares its cancel flag with an existing
    /// [`CancelHandle`]. Used when a front-end runtime constructs the
    /// cancel handle first (e.g. so the TUI's Ctrl+C handler captures
    /// it) and then attaches the event sink.
    #[must_use]
    pub fn with_cancel_handle(cancel: CancelHandle) -> Self {
        Self {
            events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            cancel,
        }
    }

    /// Borrow the share-able cancel handle. Front-end runtimes clone
    /// this so they can cancel from outside the sink owner.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Snapshot every recorded event.
    pub async fn events(&self) -> Vec<LoopEvent> {
        self.events.lock().await.clone()
    }

    /// Cause the next iteration boundary to return cancelled.
    pub fn request_cancel(&self) {
        self.cancel.request_cancel();
    }
}

#[async_trait]
impl LoopSink for MemorySink {
    async fn emit(&self, event: LoopEvent) {
        self.events.lock().await.push(event);
    }

    fn should_cancel(&self) -> bool {
        self.cancel.is_cancelled()
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
    /// Optional lifecycle hook bus. When `Some`, every tool call fires
    /// `PreToolHook` callbacks (which may `Deny` or `Warn`) and every
    /// completed call fires `PostToolHook` observers. `None` disables
    /// the surface entirely — the loop still runs unchanged.
    ///
    /// OpenHarness-inspired: PreToolUse/PostToolUse lifecycle events
    /// give plugins a hook point without touching the registry.
    pub hooks: Option<HookBus>,
    /// Optional [`Compactor`] consulted between iterations. When the
    /// strategy fires, the loop replaces the running message stack
    /// with a summarised one and emits a [`LoopEvent::Compacted`]
    /// event. `None` disables auto-compaction.
    ///
    /// OpenHarness-inspired: Auto-Compaction preserves task state
    /// across context-window pressure without dropping tool results.
    pub compactor: Option<Arc<dyn Compactor>>,
    /// Optional audit trace. When `Some` *and* `hooks` is also `Some`,
    /// every `PreToolHook::Deny` / `Warn` outcome is appended to the
    /// tamper-evident chain as a `HookDeny` / `HookWarn` entry. The
    /// args bytes never enter the chain — only their BLAKE3 hash —
    /// so hostile args cannot ride the audit channel to exfiltrate.
    ///
    /// Setting this without `hooks` does nothing; setting hooks
    /// without this skips the audit append but still fires the hook.
    pub audit: Option<AuditTrace>,
    /// Ordered list of [`PromptEnricher`]s consulted once per run.
    /// Their concatenated output lands as a leading system message
    /// before the first provider call. Empty by default; the loop
    /// runs unchanged when no enrichers are attached.
    ///
    /// OpenHarness-inspired: ambient context injection (CLAUDE.md,
    /// markdown skills, plugin operating instructions) without
    /// hard-coding the discovery into the agent crate.
    pub enrichers: Vec<Arc<dyn crate::enrich::PromptEnricher>>,
}

impl AgentLoop {
    /// Build a loop around an existing [`TurnPolicy`].
    #[must_use]
    pub fn new(policy: TurnPolicy) -> Self {
        Self {
            policy,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            fallback: Vec::new(),
            hooks: None,
            compactor: None,
            audit: None,
            enrichers: Vec::new(),
        }
    }

    /// Attach an [`AuditTrace`]. When set alongside a `HookBus`,
    /// every `Deny`/`Warn` outcome is appended to the chain.
    #[must_use]
    pub fn with_audit(mut self, audit: AuditTrace) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Append a [`PromptEnricher`]. The loop concatenates every
    /// enricher's output and prepends the result as one leading
    /// `system`-role message before the first provider call.
    /// Enrichers fire in registration order.
    #[must_use]
    pub fn with_enricher(mut self, enricher: Arc<dyn crate::enrich::PromptEnricher>) -> Self {
        self.enrichers.push(enricher);
        self
    }

    /// Attach a `HookBus`. Subsequent tool calls fire pre/post-hooks.
    /// Disabled by default to keep the no-hooks path zero-overhead.
    #[must_use]
    pub fn with_hooks(mut self, hooks: HookBus) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Attach a [`Compactor`]. The loop will consult it before every
    /// provider invocation; on a fire it rewrites the message stack
    /// and emits a [`LoopEvent::Compacted`].
    #[must_use]
    pub fn with_compactor(mut self, compactor: Arc<dyn Compactor>) -> Self {
        self.compactor = Some(compactor);
        self
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
    #[allow(clippy::cognitive_complexity)]
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

        // PromptEnricher fan-out. Runs once per loop. The combined
        // output lands as the very first system message so the
        // WindowedCompactor (which preserves the leading system
        // message verbatim) never collapses it.
        if !self.enrichers.is_empty() {
            if let Some(enrichment) = crate::enrich::collect_enrichments(&self.enrichers).await {
                messages.insert(0, enrichment);
            }
        }

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

            // Auto-Compaction: consult the compactor before every
            // provider invocation. The strategy is responsible for
            // its own idempotence — calling it on an already-compacted
            // stack is a no-op.
            if let Some(c) = self.compactor.as_ref() {
                if let Some(rec) = c.maybe_compact(&mut messages) {
                    let CompactionRecord {
                        collapsed,
                        retained,
                        before_chars,
                        after_chars,
                        ..
                    } = rec;
                    sink.emit(LoopEvent::Compacted {
                        collapsed,
                        retained,
                        before_chars,
                        after_chars,
                        turn: iterations,
                    })
                    .await;
                }
            }

            // Build the iteration's prompt from the running stack.
            let iter_prompt = Prompt {
                model: model.clone(),
                messages: messages.clone(),
                max_tokens,
                temperature,
            };

            // Call provider with fallback chain.
            let completion = match self
                .run_with_fallback(&iter_prompt, taint, session_id, sink, iterations)
                .await
            {
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

                // PreToolUse hooks. The bus may `Warn` (advisory) or
                // `Deny` (short-circuit). On deny, we synthesise a
                // tool-role error message so the model sees what
                // happened — same shape as a tool dispatch failure.
                if let Some(bus) = self.hooks.as_ref() {
                    let pre = PreToolEvent::new(tc.name.clone(), tc.args.clone()).with_taint(taint);
                    let report = bus.fire_pre(&pre).await;
                    // Args canonical-JSON, hashed for the audit chain
                    // (the chain stores only the BLAKE3 — never the
                    // raw args — so secrets in args can't leak via
                    // the receipt chain).
                    let args_bytes =
                        serde_json::to_vec(&tc.args).unwrap_or_else(|_| b"null".to_vec());
                    for w in &report.warnings {
                        sink.emit(LoopEvent::ToolWarn {
                            name: tc.name.clone(),
                            message: w.clone(),
                            turn: iterations,
                        })
                        .await;
                        if let Some(audit) = self.audit.as_ref() {
                            audit
                                .record_hook_warn(tc.name.clone(), w.clone(), &args_bytes, taint)
                                .await;
                        }
                    }
                    if let Some(reason) = report.outcome.reason().map(str::to_owned) {
                        if report.outcome.is_deny() {
                            sink.emit(LoopEvent::ToolDenied {
                                name: tc.name.clone(),
                                reason: reason.clone(),
                                turn: iterations,
                            })
                            .await;
                            if let Some(audit) = self.audit.as_ref() {
                                audit
                                    .record_hook_deny(
                                        tc.name.clone(),
                                        reason.clone(),
                                        &args_bytes,
                                        taint,
                                    )
                                    .await;
                            }
                            let body = serde_json::json!({
                                "error": "hook_denied",
                                "reason": reason,
                            });
                            let mut content = format!("[tool:{} denied] {body}", tc.name);
                            if let Some(id) = &tc.id {
                                content = format!("[tool_call_id={id}] {content}");
                            }
                            messages.push(Message {
                                role: "tool".into(),
                                content,
                            });
                            continue;
                        }
                    }
                }

                let started = std::time::Instant::now();
                let (ok, result_json) = match self
                    .policy
                    .dispatch_tool(&tc.name, tc.args.clone(), taint, 0)
                    .await
                {
                    Ok(validated) => (true, validated.into_json()),
                    Err(e) => (false, serde_json::json!({ "error": format!("{e:?}") })),
                };
                let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

                sink.emit(LoopEvent::ToolComplete {
                    name: tc.name.clone(),
                    ok,
                    result: result_json.clone(),
                    turn: iterations,
                })
                .await;

                // PostToolUse hooks fire after the schema-validated
                // result lands but before the next iteration prompts
                // the model. Advisory only.
                if let Some(bus) = self.hooks.as_ref() {
                    let post = PostToolEvent::new(tc.name.clone(), ok, result_json.clone())
                        .with_elapsed_ms(elapsed_ms)
                        .with_taint(taint);
                    bus.fire_post(&post).await;
                }

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
        iterations: u32,
    ) -> TurnResult<Completion> {
        // Sprint 9 §1 — token-streaming bridge. The provider calls
        // `on_token` once per delta (or once with the full text for
        // non-streaming drivers); we forward each into
        // `LoopEvent::Token` so the dashboard renders token-by-token.
        let token_sink = LoopTokenBridge {
            sink,
            turn: iterations,
        };
        match self
            .policy
            .run_streaming_in_session(prompt.clone(), taint, session_id, &token_sink)
            .await
        {
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
                    match fb.complete_streaming(prompt, &token_sink).await {
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

/// Bridge between the [`crate::TokenSink`] trait and a
/// [`LoopSink::emit(LoopEvent::Token)`] dispatch. Constructed for
/// the duration of one provider call so the `turn` index is stable.
struct LoopTokenBridge<'a> {
    sink: &'a dyn LoopSink,
    turn: u32,
}

#[async_trait]
impl crate::TokenSink for LoopTokenBridge<'_> {
    async fn on_token(&self, text: &str) {
        self.sink
            .emit(LoopEvent::Token {
                text: text.to_owned(),
                turn: self.turn,
            })
            .await;
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
            fn name(&self) -> &'static str {
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
            let i = self.idx.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        assert!(matches!(
            events.first(),
            Some(LoopEvent::UserSubmitted { .. })
        ));
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
        assert!(events
            .iter()
            .any(|e| matches!(e, LoopEvent::ToolStart { name, .. } if name == "echo")));
        assert!(events.iter().any(
            |e| matches!(e, LoopEvent::ToolComplete { name, ok, .. } if name == "echo" && *ok)
        ));
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

    // ── Sprint 9 §2: mid-turn cancel via share-able CancelHandle ──────────

    #[tokio::test]
    async fn cancel_handle_shared_with_sink_cancels_loop() {
        // A front-end runtime (TUI Ctrl+C, WS close, ACP cancel RPC)
        // holds a `CancelHandle` cloned from the sink. Flipping it
        // from outside cancels the next iteration boundary.
        let provider = stub_provider("hello");
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        let handle = sink.cancel_handle();
        // Cancel *from a separate task* — simulates a UI thread.
        let h = handle.clone();
        tokio::spawn(async move {
            h.request_cancel();
        })
        .await
        .unwrap();
        let outcome = loop_
            .run(
                Prompt::new("any", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "cancelled");
    }

    #[tokio::test]
    async fn sink_constructed_with_pre_built_cancel_handle_observes_flag() {
        // Inverse construction: build the handle first (the front-end
        // owns it), then attach a sink that uses it. This is the
        // production wiring for the WS server.
        let handle = CancelHandle::new();
        let sink = MemorySink::with_cancel_handle(handle.clone());
        assert!(!sink.should_cancel());
        handle.request_cancel();
        assert!(sink.should_cancel());
        assert!(handle.is_cancelled());
    }

    #[test]
    fn cancel_handle_clones_share_the_same_flag() {
        let a = CancelHandle::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        assert!(!b.is_cancelled());
        b.request_cancel();
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());
    }

    // ── Sprint 9 §1: token-level streaming through LoopSink::Token ────────

    #[tokio::test]
    async fn non_streaming_provider_still_emits_one_token_event_per_turn() {
        // The default `complete_streaming` impl on `ProviderHandle`
        // emits the full text as one token. The dashboard at least
        // sees `LoopEvent::Token` for every turn — strict superset
        // of the previous "just `Assistant`" behaviour.
        let provider = stub_provider("hello world");
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("any", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        let events = sink.events().await;
        let token_count = events
            .iter()
            .filter(|e| matches!(e, LoopEvent::Token { .. }))
            .count();
        assert_eq!(token_count, 1, "expected exactly one Token per turn");
        // The Token event must carry the same body the Assistant event
        // contains — concatenating tokens reconstructs the assistant text.
        let token_text: String = events
            .iter()
            .filter_map(|e| match e {
                LoopEvent::Token { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let assistant_text: String = events
            .iter()
            .filter_map(|e| match e {
                LoopEvent::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(token_text, assistant_text);
    }

    #[tokio::test]
    async fn streaming_provider_emits_one_token_event_per_delta() {
        // A provider that overrides `complete_streaming` to emit
        // multiple tokens drives the dashboard's per-token render
        // pipeline. The agent loop forwards each delta untouched.
        use crate::TokenSink as ATokenSink;

        struct ChunkedProvider;
        #[async_trait::async_trait]
        impl ProviderHandle for ChunkedProvider {
            fn name(&self) -> &'static str {
                "chunked"
            }
            async fn complete(&self, _p: &Prompt) -> Result<Completion, ProviderError> {
                Ok(Completion::new(
                    "hello world",
                    "chunked",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
            async fn complete_streaming(
                &self,
                _p: &Prompt,
                sink: &dyn ATokenSink,
            ) -> Result<Completion, ProviderError> {
                sink.on_token("hello ").await;
                sink.on_token("world").await;
                Ok(Completion::new(
                    "hello world",
                    "chunked",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
        }

        let provider = Arc::new(ChunkedProvider);
        let loop_ = AgentLoop::new(loop_policy(provider));
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("any", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        let events = sink.events().await;
        let tokens: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                LoopEvent::Token { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tokens, vec!["hello ".to_owned(), "world".to_owned()]);
    }

    #[tokio::test]
    async fn fallback_attempts_recorded_on_provider_error() {
        struct FailingProvider;
        #[async_trait::async_trait]
        impl ProviderHandle for FailingProvider {
            fn name(&self) -> &'static str {
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

    // ── OpenHarness-inspired hook integration tests ───────────────────────

    /// `PreToolHook::Deny` short-circuits the dispatch: the tool never
    /// runs, the loop emits a `ToolDenied` event, and an error message
    /// is appended so the model sees the denial. (OpenHarness PreToolUse.)
    #[tokio::test]
    async fn hook_deny_skips_dispatch_and_emits_event() {
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};

        struct DenyShell;
        #[async_trait::async_trait]
        impl PreToolHook for DenyShell {
            fn name(&self) -> &'static str {
                "deny_shell"
            }
            async fn on_pre_tool(&self, e: &PreToolEvent) -> HookOutcome {
                if e.tool == "echo" {
                    HookOutcome::Deny("policy: echo blocked for test".into())
                } else {
                    HookOutcome::Allow
                }
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_pre(Arc::new(DenyShell), 0);
        let loop_ = AgentLoop::new(loop_policy(provider)).with_hooks(bus);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        assert!(events.iter().any(|e| matches!(e, LoopEvent::ToolDenied { name, reason, .. } if name == "echo" && reason.contains("policy: echo blocked"))));
        // The denied tool MUST NOT have emitted a ToolComplete event.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LoopEvent::ToolComplete { name, .. } if name == "echo")),
            "denied tool should not produce ToolComplete"
        );
    }

    /// `HookOutcome::Warn` is advisory — the tool still runs, but the
    /// loop emits a `ToolWarn` event for observers.
    #[tokio::test]
    async fn hook_warn_does_not_block_dispatch() {
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};

        struct Warner;
        #[async_trait::async_trait]
        impl PreToolHook for Warner {
            fn name(&self) -> &'static str {
                "warn"
            }
            async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
                HookOutcome::Warn("be careful out there".into())
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_pre(Arc::new(Warner), 0);
        let loop_ = AgentLoop::new(loop_policy(provider)).with_hooks(bus);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        assert!(events.iter().any(
            |e| matches!(e, LoopEvent::ToolWarn { name, message, .. } if name == "echo" && message.contains("be careful"))
        ));
        // Tool DID complete (warn is advisory).
        assert!(events.iter().any(
            |e| matches!(e, LoopEvent::ToolComplete { name, ok, .. } if name == "echo" && *ok)
        ));
    }

    /// When both `hooks` and `audit` are attached, a `Deny` advances
    /// the audit chain head by one — a `HookDeny` entry lands on the
    /// chain alongside the usual `ToolDenied` `LoopEvent`.
    #[tokio::test]
    async fn hook_deny_appends_to_audit_chain() {
        use crate::audit::AuditTrace;
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};

        struct DenyAll;
        #[async_trait::async_trait]
        impl PreToolHook for DenyAll {
            fn name(&self) -> &str {
                "deny_all"
            }
            async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
                HookOutcome::Deny("blocked for test".into())
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_pre(Arc::new(DenyAll), 0);
        let audit = AuditTrace::new();
        let head_before = audit.head().await;
        let loop_ = AgentLoop::new(loop_policy(provider))
            .with_hooks(bus)
            .with_audit(audit.clone());
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        let head_after = audit.head().await;
        assert_ne!(
            head_before.to_hex(),
            head_after.to_hex(),
            "HookDeny must advance the audit chain"
        );
    }

    /// `HookWarn` outcomes also land on the audit chain (advisory but
    /// recorded for completeness).
    #[tokio::test]
    async fn hook_warn_appends_to_audit_chain() {
        use crate::audit::AuditTrace;
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};

        struct WarnTwice;
        #[async_trait::async_trait]
        impl PreToolHook for WarnTwice {
            fn name(&self) -> &str {
                "warn"
            }
            async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
                HookOutcome::Warn("careful".into())
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_pre(Arc::new(WarnTwice), 0);
        let audit = AuditTrace::new();
        let head_before = audit.head().await;
        let loop_ = AgentLoop::new(loop_policy(provider))
            .with_hooks(bus)
            .with_audit(audit.clone());
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        let head_after = audit.head().await;
        assert_ne!(
            head_before.to_hex(),
            head_after.to_hex(),
            "HookWarn must advance the audit chain"
        );
    }

    /// Without `with_audit`, the loop still functions and emits the
    /// `LoopEvent::ToolDenied`. Confirms the audit append is optional.
    #[tokio::test]
    async fn audit_is_optional_for_hook_dispatch() {
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};

        struct DenyAll;
        #[async_trait::async_trait]
        impl PreToolHook for DenyAll {
            fn name(&self) -> &str {
                "deny_all"
            }
            async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
                HookOutcome::Deny("nope".into())
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_pre(Arc::new(DenyAll), 0);
        let loop_ = AgentLoop::new(loop_policy(provider)).with_hooks(bus);
        // No `with_audit(...)`.
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        let events = sink.events().await;
        assert!(events
            .iter()
            .any(|e| matches!(e, LoopEvent::ToolDenied { .. })));
    }

    // ── OpenHarness-inspired PromptEnricher integration tests ─────────────

    /// A [`PromptEnricher`] prepends one leading system message
    /// before the first provider call. The provider sees the
    /// enrichment in its prompt input.
    #[tokio::test]
    async fn enricher_prepends_leading_system_message() {
        use crate::enrich::PromptEnricher;
        use async_trait::async_trait;
        use std::sync::Mutex;

        struct Capture {
            seen: Mutex<Vec<Prompt>>,
        }
        #[async_trait::async_trait]
        impl ProviderHandle for Capture {
            fn name(&self) -> &str {
                "capture"
            }
            async fn complete(&self, p: &Prompt) -> Result<Completion, ProviderError> {
                self.seen.lock().unwrap().push(p.clone());
                Ok(Completion::new(
                    "ok",
                    "capture",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
        }
        let capture = Arc::new(Capture {
            seen: Mutex::new(Vec::new()),
        });

        struct CtxEnricher;
        #[async_trait]
        impl PromptEnricher for CtxEnricher {
            fn name(&self) -> &str {
                "ctx-test"
            }
            async fn enrich(&self) -> Option<String> {
                Some("USE PROJECT NORMS".into())
            }
        }

        let loop_ = AgentLoop::new(loop_policy(capture.clone() as Arc<dyn ProviderHandle>))
            .with_enricher(Arc::new(CtxEnricher));
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("capture", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");

        let seen = capture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let first_msg = &seen[0].messages[0];
        assert_eq!(first_msg.role, "system");
        assert!(first_msg.content.contains("USE PROJECT NORMS"));
        assert!(first_msg
            .content
            .contains("<!-- prompt-enricher: ctx-test -->"));
    }

    /// Two enrichers concatenate in registration order with a
    /// divider, still as one leading system block.
    #[tokio::test]
    async fn multiple_enrichers_compose() {
        use crate::enrich::PromptEnricher;
        use std::sync::Mutex;

        struct Capture {
            seen: Mutex<Vec<Prompt>>,
        }
        #[async_trait::async_trait]
        impl ProviderHandle for Capture {
            fn name(&self) -> &str {
                "capture"
            }
            async fn complete(&self, p: &Prompt) -> Result<Completion, ProviderError> {
                self.seen.lock().unwrap().push(p.clone());
                Ok(Completion::new(
                    "ok",
                    "capture",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
        }
        let capture = Arc::new(Capture {
            seen: Mutex::new(Vec::new()),
        });

        struct E(&'static str, &'static str);
        #[async_trait]
        impl PromptEnricher for E {
            fn name(&self) -> &str {
                self.0
            }
            async fn enrich(&self) -> Option<String> {
                Some(self.1.into())
            }
        }

        let loop_ = AgentLoop::new(loop_policy(capture.clone() as Arc<dyn ProviderHandle>))
            .with_enricher(Arc::new(E("first", "AAA")))
            .with_enricher(Arc::new(E("second", "BBB")));
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("capture", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");

        let seen = capture.seen.lock().unwrap();
        let body = &seen[0].messages[0].content;
        let pos_aaa = body.find("AAA").unwrap();
        let pos_bbb = body.find("BBB").unwrap();
        assert!(pos_aaa < pos_bbb);
        assert!(body.contains("<!-- prompt-enricher: first -->"));
        assert!(body.contains("<!-- prompt-enricher: second -->"));
    }

    /// An enricher that returns `None` is silently skipped — the
    /// loop runs as if no enrichers were attached.
    #[tokio::test]
    async fn opt_out_enricher_does_not_insert_anything() {
        use crate::enrich::PromptEnricher;
        use std::sync::Mutex;

        struct Capture {
            seen: Mutex<Vec<Prompt>>,
        }
        #[async_trait::async_trait]
        impl ProviderHandle for Capture {
            fn name(&self) -> &str {
                "capture"
            }
            async fn complete(&self, p: &Prompt) -> Result<Completion, ProviderError> {
                self.seen.lock().unwrap().push(p.clone());
                Ok(Completion::new(
                    "ok",
                    "capture",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
        }
        let capture = Arc::new(Capture {
            seen: Mutex::new(Vec::new()),
        });

        struct Silent;
        #[async_trait]
        impl PromptEnricher for Silent {
            fn name(&self) -> &str {
                "silent"
            }
            async fn enrich(&self) -> Option<String> {
                None
            }
        }

        let loop_ = AgentLoop::new(loop_policy(capture.clone() as Arc<dyn ProviderHandle>))
            .with_enricher(Arc::new(Silent));
        let sink = MemorySink::new();
        loop_
            .run(
                Prompt::new("capture", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");

        let seen = capture.seen.lock().unwrap();
        // The original prompt had exactly one user message; no
        // enrichment was injected.
        assert_eq!(seen[0].messages.len(), 1);
        assert_eq!(seen[0].messages[0].role, "user");
    }

    /// Auto-Compaction fires between iterations when the running
    /// stack exceeds the budget; the loop emits a `Compacted` event
    /// and the next provider call sees a smaller stack.
    #[tokio::test]
    async fn auto_compaction_fires_between_iterations() {
        use crate::compaction::WindowedCompactor;
        use std::sync::Arc;

        // Tight budget so a single big assistant message triggers compaction.
        let compactor: Arc<dyn crate::Compactor> = Arc::new(
            WindowedCompactor::defaults()
                .with_budget_chars(200)
                .with_keep_tail(1),
        );

        // Script: turn 1 produces a big assistant message (no tool),
        // turn 2 stops.
        let big = "x".repeat(2_000);
        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    format!("<tool name=\"echo\">{{\"text\":\"{big}\"}}</tool>"),
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let loop_ = AgentLoop::new(loop_policy(provider)).with_compactor(compactor);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new(
                    "scripted",
                    vec![
                        Message::new("system", "sys"),
                        // Seed prior history so there's something to collapse.
                        Message::new("user", "x".repeat(500)),
                        Message::new("assistant", "y".repeat(500)),
                        Message::new("user", "go"),
                    ],
                ),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "stop");
        let events = sink.events().await;
        let fired = events
            .iter()
            .any(|e| matches!(e, LoopEvent::Compacted { collapsed, .. } if *collapsed > 0));
        assert!(fired, "Compacted event should fire");
    }

    /// `PostToolHook` observes every completed dispatch — exercised
    /// here by an AtomicUsize counter that increments per fire.
    #[tokio::test]
    async fn post_hook_observes_completed_dispatch() {
        use gauss_hooks::{HookBus, PostToolEvent, PostToolHook};
        use std::sync::atomic::{AtomicUsize, Ordering};

        static SEEN: AtomicUsize = AtomicUsize::new(0);
        struct Counter;
        #[async_trait::async_trait]
        impl PostToolHook for Counter {
            fn name(&self) -> &'static str {
                "counter"
            }
            async fn on_post_tool(&self, _e: &PostToolEvent) {
                SEEN.fetch_add(1, Ordering::SeqCst);
            }
        }

        let provider = Arc::new(ScriptProvider::new(
            "scripted",
            vec![
                Completion::new(
                    "<tool name=\"echo\">{\"text\":\"hi\"}</tool>",
                    "scripted",
                    "tool",
                    TokenCount::new(1, 1),
                ),
                Completion::new("done", "scripted", "stop", TokenCount::new(1, 1)),
            ],
        ));
        let bus = HookBus::new();
        bus.register_post(Arc::new(Counter), 0);
        SEEN.store(0, Ordering::SeqCst);
        let loop_ = AgentLoop::new(loop_policy(provider)).with_hooks(bus);
        let sink = MemorySink::new();
        let outcome = loop_
            .run(
                Prompt::new("scripted", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("ok");
        assert_eq!(outcome.stop_reason, "stop");
        assert_eq!(SEEN.load(Ordering::SeqCst), 1);
    }
}
