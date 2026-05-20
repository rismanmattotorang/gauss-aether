//! Canonical wire translation from [`gaussclaw_agent::LoopEvent`] to
//! the dashboard frame shape.
//!
//! The dashboard's `app.js` consumes JSON envelopes with a `type`
//! discriminator (`tool.start`, `tool.complete`, `token`, `assistant`,
//! `receipt`, …). The agent crate's `LoopEvent` uses `kind` with
//! snake_case names. This module is the canonical translation —
//! both sides agree on the shape here, and the agent loop never
//! needs to know about the dashboard.
//!
//! ## Why a separate module?
//!
//! Two reasons:
//!
//! 1. **Stable wire surface.** The dashboard's frame shape predates
//!    the agent loop's `LoopEvent` enum; we keep the dashboard side
//!    untouched by translating in the web crate.
//! 2. **Test in isolation.** With the translation in one function we
//!    can unit-test every variant against the JSON the dashboard
//!    expects, without standing up a WebSocket.

#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gaussclaw_agent::{CancelHandle, LoopEvent, LoopSink};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Translate one [`LoopEvent`] into the JSON envelope the dashboard
/// chat pane expects.
///
/// Returns `None` for variants that have no dashboard rendering
/// (silently dropped — the dashboard ignores what it doesn't
/// recognise).
#[must_use]
pub fn loop_event_to_wire(event: &LoopEvent) -> Option<Value> {
    match event {
        LoopEvent::UserSubmitted { text, turn } => Some(json!({
            "type": "user",
            "text": text,
            "turn": turn,
        })),
        LoopEvent::Token { text, turn } => Some(json!({
            "type": "token",
            "text": text,
            "turn": turn,
        })),
        LoopEvent::Assistant { text, turn } => Some(json!({
            "type": "assistant",
            "text": text,
            "turn": turn,
        })),
        LoopEvent::ToolStart { name, args, turn } => Some(json!({
            "type": "tool.start",
            "tool": name,
            "args": args,
            "turn": turn,
        })),
        LoopEvent::ToolComplete {
            name,
            ok,
            result,
            turn,
        } => Some(json!({
            "type": "tool.complete",
            "tool": name,
            "ok": ok,
            "result": result,
            "turn": turn,
        })),
        LoopEvent::ToolDenied { name, reason, turn } => Some(json!({
            "type": "tool.denied",
            "tool": name,
            "reason": reason,
            "turn": turn,
        })),
        LoopEvent::ToolWarn {
            name,
            message,
            turn,
        } => Some(json!({
            "type": "tool.warn",
            "tool": name,
            "message": message,
            "turn": turn,
        })),
        LoopEvent::FallbackAttempt {
            from_provider,
            to_provider,
            reason,
        } => Some(json!({
            "type": "fallback",
            "from": from_provider,
            "to": to_provider,
            "reason": reason,
        })),
        LoopEvent::Compacted {
            collapsed,
            retained,
            before_chars,
            after_chars,
            turn,
        } => Some(json!({
            "type": "compacted",
            "collapsed": collapsed,
            "retained": retained,
            "before_chars": before_chars,
            "after_chars": after_chars,
            "turn": turn,
        })),
        LoopEvent::Done {
            stop_reason,
            iterations,
        } => Some(json!({
            "type": "done",
            "stop_reason": stop_reason,
            "iterations": iterations,
        })),
        _ => None,
    }
}

// ─── LoopSink → WebSocket bridge ──────────────────────────────────────────

/// Trait abstraction over "something that can ship a JSON envelope
/// over the wire". The chat WebSocket implements this directly; tests
/// implement it with an in-memory `Vec<Value>` capture.
#[async_trait::async_trait]
pub trait WireOutbox: Send + Sync {
    /// Send one envelope. Returns `false` when the outbox is closed
    /// (connection lost) — the sink propagates the close to the loop
    /// via [`LoopSink::should_cancel`].
    async fn send(&self, frame: Value) -> bool;
}

/// `LoopSink` that translates every `LoopEvent` via
/// [`loop_event_to_wire`] and ships it through a [`WireOutbox`].
///
/// Used by `chat_socket` to stream loop activity to the dashboard;
/// used by tests with an in-memory outbox.
pub struct WireLoopSink {
    outbox: Arc<dyn WireOutbox>,
    /// Set to `true` the moment the outbox reports a failed send;
    /// the agent loop checks this between iterations and returns
    /// `LoopOutcome::Cancelled` so we never run a turn against a
    /// closed socket.
    closed: AtomicBool,
    /// Optional external cancel surface — flipped by the
    /// `chat_socket` task when the WebSocket emits a `Close` frame
    /// mid-turn (Sprint 10 §8). When set, `should_cancel()` returns
    /// `true` even before the next outbound send fails. `None` keeps
    /// the legacy "cancel on failed send only" semantics.
    cancel: Option<CancelHandle>,
}

impl WireLoopSink {
    /// Wrap an outbox.
    #[must_use]
    pub fn new(outbox: Arc<dyn WireOutbox>) -> Self {
        Self {
            outbox,
            closed: AtomicBool::new(false),
            cancel: None,
        }
    }

    /// Attach an external [`CancelHandle`]. Sprint 10 §8 — the
    /// `chat_socket` task clones a handle into its `tokio::select!`
    /// arm so that an inbound WebSocket `Close` flips this flag and
    /// the in-flight agent loop winds down at the next boundary.
    #[must_use]
    pub fn with_cancel_handle(mut self, handle: CancelHandle) -> Self {
        self.cancel = Some(handle);
        self
    }
}

#[async_trait::async_trait]
impl LoopSink for WireLoopSink {
    async fn emit(&self, event: LoopEvent) {
        if self.should_cancel() {
            return;
        }
        if let Some(frame) = loop_event_to_wire(&event) {
            // The dashboard already accepts both bare `{type: …}` envelopes
            // and `{ok: true, data: {…}}` envelopes; emit the bare shape so
            // app.js's existing `payload.type === '…'` dispatch fires
            // unchanged.
            if !self.outbox.send(frame).await {
                self.closed.store(true, Ordering::SeqCst);
            }
        }
    }

    fn should_cancel(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
            || self.cancel.as_ref().is_some_and(CancelHandle::is_cancelled)
    }
}

/// In-memory outbox used by the chat-socket integration tests.
#[derive(Default)]
pub struct CaptureOutbox {
    frames: Mutex<Vec<Value>>,
}

impl CaptureOutbox {
    /// Snapshot every received frame.
    pub async fn frames(&self) -> Vec<Value> {
        self.frames.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl WireOutbox for CaptureOutbox {
    async fn send(&self, frame: Value) -> bool {
        self.frames.lock().await.push(frame);
        true
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_translates_to_type_assistant() {
        let frame = loop_event_to_wire(&LoopEvent::Assistant {
            text: "hi".into(),
            turn: 1,
        })
        .expect("some");
        assert_eq!(frame["type"], "assistant");
        assert_eq!(frame["text"], "hi");
        assert_eq!(frame["turn"], 1);
    }

    #[test]
    fn tool_start_carries_args() {
        let frame = loop_event_to_wire(&LoopEvent::ToolStart {
            name: "echo".into(),
            args: json!({ "text": "x" }),
            turn: 2,
        })
        .expect("some");
        assert_eq!(frame["type"], "tool.start");
        assert_eq!(frame["tool"], "echo");
        assert_eq!(frame["args"]["text"], "x");
    }

    #[test]
    fn tool_complete_carries_ok_and_result() {
        let frame = loop_event_to_wire(&LoopEvent::ToolComplete {
            name: "echo".into(),
            ok: true,
            result: json!({ "echo": "x" }),
            turn: 2,
        })
        .expect("some");
        assert_eq!(frame["type"], "tool.complete");
        assert_eq!(frame["ok"], true);
        assert_eq!(frame["result"]["echo"], "x");
    }

    #[test]
    fn tool_denied_renders_with_reason() {
        let frame = loop_event_to_wire(&LoopEvent::ToolDenied {
            name: "shell".into(),
            reason: "policy: blocked".into(),
            turn: 1,
        })
        .expect("some");
        assert_eq!(frame["type"], "tool.denied");
        assert_eq!(frame["tool"], "shell");
        assert_eq!(frame["reason"], "policy: blocked");
    }

    #[test]
    fn tool_warn_renders_with_message() {
        let frame = loop_event_to_wire(&LoopEvent::ToolWarn {
            name: "http_get".into(),
            message: "rate limit".into(),
            turn: 3,
        })
        .expect("some");
        assert_eq!(frame["type"], "tool.warn");
        assert_eq!(frame["message"], "rate limit");
    }

    #[test]
    fn compacted_carries_before_after_chars() {
        let frame = loop_event_to_wire(&LoopEvent::Compacted {
            collapsed: 4,
            retained: 8,
            before_chars: 30_000,
            after_chars: 12_000,
            turn: 5,
        })
        .expect("some");
        assert_eq!(frame["type"], "compacted");
        assert_eq!(frame["collapsed"], 4);
        assert_eq!(frame["retained"], 8);
        assert_eq!(frame["before_chars"], 30_000);
        assert_eq!(frame["after_chars"], 12_000);
    }

    #[test]
    fn fallback_attempt_uses_short_field_names() {
        let frame = loop_event_to_wire(&LoopEvent::FallbackAttempt {
            from_provider: "anthropic".into(),
            to_provider: "openai".into(),
            reason: "transport down".into(),
        })
        .expect("some");
        assert_eq!(frame["type"], "fallback");
        assert_eq!(frame["from"], "anthropic");
        assert_eq!(frame["to"], "openai");
    }

    #[test]
    fn done_carries_stop_reason() {
        let frame = loop_event_to_wire(&LoopEvent::Done {
            stop_reason: "stop".into(),
            iterations: 7,
        })
        .expect("some");
        assert_eq!(frame["type"], "done");
        assert_eq!(frame["stop_reason"], "stop");
        assert_eq!(frame["iterations"], 7);
    }

    #[test]
    fn token_translates_to_type_token() {
        let frame = loop_event_to_wire(&LoopEvent::Token {
            text: "hel".into(),
            turn: 1,
        })
        .expect("some");
        assert_eq!(frame["type"], "token");
        assert_eq!(frame["text"], "hel");
    }

    #[test]
    fn user_submitted_translates_to_type_user() {
        let frame = loop_event_to_wire(&LoopEvent::UserSubmitted {
            text: "hi".into(),
            turn: 0,
        })
        .expect("some");
        assert_eq!(frame["type"], "user");
        assert_eq!(frame["text"], "hi");
    }

    // ── Sprint 10 §8: WireLoopSink cancel-handle wiring ─────────────────

    #[tokio::test]
    async fn sink_without_cancel_handle_only_cancels_on_failed_send() {
        // Legacy behaviour preserved: a sink with no external cancel
        // handle still cancels when the outbox fails, but stays
        // running as long as sends succeed.
        let outbox: Arc<dyn WireOutbox> = Arc::new(CaptureOutbox::default());
        let sink = WireLoopSink::new(outbox);
        assert!(!sink.should_cancel());
        sink.emit(LoopEvent::Token {
            text: "hi".into(),
            turn: 0,
        })
        .await;
        assert!(!sink.should_cancel());
    }

    #[tokio::test]
    async fn external_cancel_handle_flips_should_cancel() {
        // Sprint 10 §8 — the `chat_socket` task clones the cancel
        // handle into its `tokio::select!`. Flipping it must be
        // observable through the sink without needing a failed send.
        let outbox: Arc<dyn WireOutbox> = Arc::new(CaptureOutbox::default());
        let handle = CancelHandle::new();
        let sink = WireLoopSink::new(outbox).with_cancel_handle(handle.clone());
        assert!(!sink.should_cancel());
        handle.request_cancel();
        assert!(sink.should_cancel());
    }

    #[tokio::test]
    async fn cancelled_sink_drops_subsequent_emits() {
        let outbox = Arc::new(CaptureOutbox::default());
        let outbox_dyn: Arc<dyn WireOutbox> = outbox.clone();
        let handle = CancelHandle::new();
        let sink = WireLoopSink::new(outbox_dyn).with_cancel_handle(handle.clone());
        sink.emit(LoopEvent::Token {
            text: "first".into(),
            turn: 0,
        })
        .await;
        handle.request_cancel();
        sink.emit(LoopEvent::Token {
            text: "second".into(),
            turn: 1,
        })
        .await;
        // Only the first token made it through.
        let frames = outbox.frames().await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["text"], "first");
    }

    #[tokio::test]
    async fn failed_outbox_send_still_sets_closed_flag() {
        // A separate failure path from the external handle: when the
        // outbox itself reports a failed send, the sink's internal
        // `closed` flag flips. Both paths must work independently.
        struct AlwaysFail;
        #[async_trait::async_trait]
        impl WireOutbox for AlwaysFail {
            async fn send(&self, _: Value) -> bool {
                false
            }
        }
        let outbox: Arc<dyn WireOutbox> = Arc::new(AlwaysFail);
        let sink = WireLoopSink::new(outbox);
        assert!(!sink.should_cancel());
        sink.emit(LoopEvent::Token {
            text: "x".into(),
            turn: 0,
        })
        .await;
        assert!(sink.should_cancel());
    }
}
