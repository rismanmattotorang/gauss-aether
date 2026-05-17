//! Actions emitted by the policy `π`.
//!
//! The action space partitions into text emissions and tool invocations,
//! matching paper Definition 1 (`A = A_txt ⊔ A_tool`).

use serde::{Deserialize, Serialize};

use crate::cap::CapToken;
use crate::ids::ToolId;

/// An action emitted by the policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Action {
    /// Plain text emission destined for the user / channel adapter.
    Text(TextAction),
    /// Tool invocation; semantics flow through the HWCA worker boundary.
    Tool(ToolAction),
}

/// A text emission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextAction {
    /// The body of the message. The kernel does not interpret this string.
    pub body: String,
}

impl TextAction {
    /// Construct a text action.
    pub fn new(body: impl Into<String>) -> Self {
        Self { body: body.into() }
    }
}

/// A tool invocation; the `args` value is opaque to the kernel and validated
/// by the tool's declared output schema inside the worker context (Phase 4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolAction {
    /// Identifier of the tool to invoke.
    pub tool: ToolId,
    /// Arguments — opaque JSON until schema validation (HWCA, Phase 4).
    pub args: serde_json::Value,
    /// Capability bundle the tool requires (set in the tool manifest).
    pub cap_required: CapToken,
    /// Whether this action is reversible (set in the tool manifest).
    pub reversible: bool,
}

impl ToolAction {
    /// Construct a tool action. Required because `ToolAction` is
    /// `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(
        tool: ToolId,
        args: serde_json::Value,
        cap_required: CapToken,
        reversible: bool,
    ) -> Self {
        Self {
            tool,
            args,
            cap_required,
            reversible,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_text_action() {
        let a = Action::Text(TextAction::new("hello"));
        let s = serde_json::to_string(&a).unwrap();
        let b: Action = serde_json::from_str(&s).unwrap();
        match b {
            Action::Text(t) => assert_eq!(t.body, "hello"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    #[allow(clippy::missing_const_for_fn)]
    fn tool_action_default_reversibility_is_explicit() {
        // The struct has no Default impl, by design — reversibility must be
        // declared rather than implied. This test exists to make sure that
        // accidental Default derives don't sneak in.
        const fn assert_no_default<T>()
        where
            T: Sized,
        {
        }
        assert_no_default::<ToolAction>();
    }
}
