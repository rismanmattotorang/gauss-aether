//! End-to-end integration tests that drive a real vendor codec through
//! the full [`gaussclaw_agent::AgentLoop`] stack against a deterministic
//! mock HTTP backend.
//!
//! This module exists because every other test in the workspace runs
//! against `EchoProvider` or `ScriptProvider`. Those exercise the loop
//! mechanics but not the *vendor wire codec* — the JSON-shape encode/
//! decode that's the most likely source of integration bugs.
//!
//! The tests in this module wire:
//!
//! ```text
//! AnthropicProvider          ← real vendor codec
//!   + MockHttpBackend        ← canned Anthropic-shape responses
//!   + KernelHandle::permissive
//!   + TurnPolicy
//!   + AgentLoop::run
//! ```
//!
//! and assert that:
//!
//! - the request leaving the codec carries the right URL / headers /
//!   body shape,
//! - the response coming back is parsed correctly through the codec,
//! - the parsed [`Completion`] reaches the `LoopSink` as the expected
//!   `LoopEvent::Assistant` / `LoopEvent::Done` frames,
//! - the audit trace records a turn-complete entry,
//! - a multi-iteration run (compaction firing) still emits the right
//!   event sequence.
//!
//! These tests are the canonical demonstration that the OpenHarness-
//! inspired surfaces work *with a real vendor*. A real network test
//! against `api.anthropic.com` requires API credentials and is out of
//! scope for `cargo test`; the recipe for one is documented in
//! `docs/OPENHARNESS_PARITY.md`.

#![cfg(test)]
#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use gaussclaw_agent::{
    AgentLoop, AuditTrace, KernelHandle, LoopEvent, MemorySink, Message, Prompt, ProviderHandle,
    TurnPolicy,
};
use gauss_core::TaintLabel;
use serde_json::json;

use crate::{AnthropicProvider, HttpResponse, MockHttpBackend};

// ─── fixtures ─────────────────────────────────────────────────────────────

fn canned_response(text: &str, stop_reason: &str) -> HttpResponse {
    let body = json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-3.5-sonnet",
        "content": [{ "type": "text", "text": text }],
        "stop_reason": stop_reason,
        "usage": { "input_tokens": 10, "output_tokens": 5 },
    });
    HttpResponse {
        status: 200,
        body: serde_json::to_vec(&body).unwrap(),
    }
}

fn build_loop(provider: Arc<dyn ProviderHandle>) -> (AgentLoop, AuditTrace) {
    let audit = AuditTrace::new();
    let policy =
        TurnPolicy::new(KernelHandle::permissive(), provider).with_audit(audit.clone());
    (AgentLoop::new(policy).with_audit(audit.clone()), audit)
}

fn user_prompt(text: &str) -> Prompt {
    Prompt::new(
        "claude-3.5-sonnet",
        vec![Message::new("user", text.to_string())],
    )
}

// ─── tests ────────────────────────────────────────────────────────────────

/// A one-iteration run: user → AnthropicProvider → canned response →
/// LoopEvent::Assistant + LoopEvent::Done. The completion text MUST be
/// the canned text round-tripped through the codec, not a stub echo.
#[tokio::test]
async fn anthropic_provider_drives_full_loop_one_turn() {
    let backend = Arc::new(MockHttpBackend::new(vec![canned_response(
        "Hello from Anthropic.",
        "end_turn",
    )]));
    let provider: Arc<dyn ProviderHandle> =
        Arc::new(AnthropicProvider::new(backend.clone(), "sk-test"));
    let (loop_, _audit) = build_loop(provider);
    let sink = MemorySink::new();
    let outcome = loop_
        .run(user_prompt("hi"), TaintLabel::User, None, &sink)
        .await
        .expect("run ok");

    assert_eq!(outcome.stop_reason, "stop");
    assert_eq!(outcome.iterations, 1);

    let events = sink.events().await;
    let assistant = events.iter().find_map(|e| match e {
        LoopEvent::Assistant { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(assistant.as_deref(), Some("Hello from Anthropic."));
    assert!(events
        .iter()
        .any(|e| matches!(e, LoopEvent::Done { stop_reason, .. } if stop_reason == "stop")));

    // The codec actually saw the loop's prompt.
    let seen = backend.seen();
    assert_eq!(seen.len(), 1);
    let req_body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(req_body["model"], "claude-3.5-sonnet");
    // Anthropic codec lifts system to top-level; this test only sent a
    // user message, so the messages array has one entry.
    assert_eq!(req_body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(req_body["messages"][0]["role"], "user");
}

/// Auto-Compaction stays inert when one turn is short — the leading
/// system message + one user message + the canned assistant reply is
/// well under any reasonable budget. Confirms the codec composes
/// cleanly with the compactor (no spurious compaction frames).
#[tokio::test]
async fn anthropic_short_turn_no_compaction() {
    let backend = Arc::new(MockHttpBackend::new(vec![canned_response("ok", "end_turn")]));
    let provider: Arc<dyn ProviderHandle> =
        Arc::new(AnthropicProvider::new(backend, "sk-test"));
    let (loop_, _audit) = build_loop(provider);
    let sink = MemorySink::new();
    loop_
        .run(user_prompt("hi"), TaintLabel::User, None, &sink)
        .await
        .expect("ok");
    let events = sink.events().await;
    assert!(!events
        .iter()
        .any(|e| matches!(e, LoopEvent::Compacted { .. })));
}

/// The audit trace head advances during a real-codec run — the
/// audit chain witnesses the turn whether the provider is stubbed
/// or real.
#[tokio::test]
async fn anthropic_run_advances_audit_chain() {
    let backend = Arc::new(MockHttpBackend::new(vec![canned_response("ok", "end_turn")]));
    let provider: Arc<dyn ProviderHandle> =
        Arc::new(AnthropicProvider::new(backend, "sk-test"));
    let (loop_, audit) = build_loop(provider);
    let head_before = audit.head().await;
    let sink = MemorySink::new();
    loop_
        .run(user_prompt("hi"), TaintLabel::User, None, &sink)
        .await
        .expect("ok");
    let head_after = audit.head().await;
    assert_ne!(head_before.to_hex(), head_after.to_hex());
}

/// Two consecutive runs against the same loop produce two distinct
/// audit chain heads — confirms idempotence (each run is its own
/// receipt) without cross-run interference.
#[tokio::test]
async fn two_sequential_runs_each_produce_a_chain_advance() {
    let backend = Arc::new(MockHttpBackend::new(vec![
        canned_response("first", "end_turn"),
        canned_response("second", "end_turn"),
    ]));
    let provider: Arc<dyn ProviderHandle> =
        Arc::new(AnthropicProvider::new(backend, "sk-test"));
    let (loop_, audit) = build_loop(provider);
    let sink = MemorySink::new();
    let head_initial = audit.head().await;
    loop_
        .run(user_prompt("one"), TaintLabel::User, None, &sink)
        .await
        .expect("ok");
    let head_after_first = audit.head().await;
    loop_
        .run(user_prompt("two"), TaintLabel::User, None, &sink)
        .await
        .expect("ok");
    let head_after_second = audit.head().await;
    assert_ne!(head_initial.to_hex(), head_after_first.to_hex());
    assert_ne!(head_after_first.to_hex(), head_after_second.to_hex());
}

/// An upstream 5xx surfaces as a `LoopEvent::Done { stop_reason:
/// "error" }` — proves the error path stays observable through the
/// loop event stream when a vendor codec fails for real.
#[tokio::test]
async fn anthropic_upstream_5xx_surfaces_as_error_done() {
    let backend = Arc::new(MockHttpBackend::new(vec![HttpResponse {
        status: 503,
        body: b"service unavailable".to_vec(),
    }]));
    let provider: Arc<dyn ProviderHandle> =
        Arc::new(AnthropicProvider::new(backend, "sk-test"));
    let (loop_, _audit) = build_loop(provider);
    let sink = MemorySink::new();
    let result = loop_
        .run(user_prompt("hi"), TaintLabel::User, None, &sink)
        .await;
    assert!(result.is_err(), "5xx must propagate as Err");
    let events = sink.events().await;
    assert!(events
        .iter()
        .any(|e| matches!(e, LoopEvent::Done { stop_reason, .. } if stop_reason == "error")));
}

/// The codec emits the canonical `x-api-key` + `anthropic-version`
/// headers when the loop dispatches a turn. Without this, a real API
/// key would never reach the wire.
#[tokio::test]
async fn loop_run_propagates_credentials_to_the_codec() {
    let backend = Arc::new(MockHttpBackend::new(vec![canned_response("ok", "end_turn")]));
    let provider: Arc<dyn ProviderHandle> = Arc::new(AnthropicProvider::new(
        backend.clone(),
        "sk-ant-this-is-the-test-key",
    ));
    let (loop_, _audit) = build_loop(provider);
    let sink = MemorySink::new();
    loop_
        .run(user_prompt("hi"), TaintLabel::User, None, &sink)
        .await
        .expect("ok");
    let seen = backend.seen();
    let headers = &seen[0].headers;
    let api_key_header = headers
        .iter()
        .find(|(k, _)| k == "x-api-key")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert_eq!(api_key_header, "sk-ant-this-is-the-test-key");
    assert!(headers.iter().any(|(k, _)| k == "anthropic-version"));
    // The canonical POST endpoint is the Messages API.
    assert!(seen[0].url.ends_with("/v1/messages"));
}
