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

use gaussclaw_agent::LoopEvent;
use serde_json::{json, Value};

/// Translate one [`LoopEvent`] into the JSON envelope the dashboard
/// chat pane expects. Returns `None` for variants that have no
/// dashboard rendering (silently dropped — the dashboard ignores
/// what it doesn't recognise).
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
}
