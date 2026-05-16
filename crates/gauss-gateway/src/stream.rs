//! Server-sent event shapes (`text/event-stream`).
//!
//! The gateway emits one SSE event per chain append + one event per SAG
//! decision + one event per approval queue change. Clients reconnect by
//! re-issuing the last `chain_index` they saw.

use gauss_core::Action;
use serde::{Deserialize, Serialize};

/// One SSE event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum StreamEvent {
    /// A turn appended to the chain.
    TurnAppended {
        /// Chain index of the append.
        chain_index: u64,
        /// Chain length after the append.
        chain_length: u64,
        /// Actions that were appended.
        actions: Vec<Action>,
    },
    /// SAG decision recorded.
    SagDecision {
        /// Chain index of the covered append.
        chain_index: u64,
        /// Decision detail (subset of `gauss_sag::SagDecisionRecord`).
        tool: String,
        /// Whether the action proceeded.
        proceeded: bool,
        /// Approver identity, if any.
        approver: Option<String>,
    },
    /// Health-engine outcome.
    HealthChanged {
        /// `ok` / `warning` / `failing`.
        verdict: String,
        /// Failing invariant id, if any.
        failing_invariant: Option<String>,
    },
    /// Keepalive ping. The gateway emits one every 30 s by default.
    Ping,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_event_round_trips() {
        let e = StreamEvent::TurnAppended {
            chain_index: 0,
            chain_length: 1,
            actions: Vec::new(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"kind\":\"turn_appended\""));
        let back: StreamEvent = serde_json::from_str(&s).unwrap();
        matches!(back, StreamEvent::TurnAppended { .. });
    }

    #[test]
    fn ping_event_is_compact() {
        let e = StreamEvent::Ping;
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, "{\"kind\":\"ping\"}");
    }
}
