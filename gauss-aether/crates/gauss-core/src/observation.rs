//! Observations — incoming data crossing into the agent's reasoning context.
//!
//! Every observation carries a taint label. The kernel joins observation
//! taints into the turn taint per Axiom 6.

use serde::{Deserialize, Serialize};

use crate::taint::TaintLabel;

/// An observation that the agent will see.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Observation {
    /// Where this observation came from.
    pub source: ObservationSource,
    /// Taint of the observation's source.
    pub taint: TaintLabel,
    /// Payload body — opaque JSON.
    pub body: serde_json::Value,
}

impl Observation {
    /// Construct an observation. This explicit constructor exists because the
    /// struct is `#[non_exhaustive]` and therefore cannot be built with a
    /// struct literal from outside this crate.
    #[must_use]
    pub const fn new(
        source: ObservationSource,
        taint: TaintLabel,
        body: serde_json::Value,
    ) -> Self {
        Self {
            source,
            taint,
            body,
        }
    }
}

/// Provenance of an observation. The kernel uses this for `declass` lookups.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ObservationSource {
    /// A user message routed through a channel adapter.
    User {
        /// Channel adapter identifier (e.g. `"telegram"`).
        channel: String,
    },
    /// Output of a tool invocation that has crossed the worker boundary.
    Tool {
        /// Identifier of the originating tool.
        tool: String,
    },
    /// A scheduled trigger fired by the daemon plane.
    Schedule {
        /// Identifier of the schedule that fired.
        schedule_id: String,
    },
    /// A canvas event from the A2UI layer.
    Canvas {
        /// Widget identifier the event came from.
        widget_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_taint_default() {
        let obs = Observation {
            source: ObservationSource::User {
                channel: "telegram".into(),
            },
            taint: TaintLabel::User,
            body: serde_json::json!({"text": "hi"}),
        };
        assert_eq!(obs.taint, TaintLabel::User);
    }
}
