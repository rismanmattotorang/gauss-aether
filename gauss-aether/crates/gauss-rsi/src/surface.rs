//! Surface DTOs — REST/WebSocket API and TUI panel spec (paper §IV.H,
//! Appendices D and E).
//!
//! The UI layer is observability and the human-oversight cadence SAHOO's
//! deployment guidance requires. It is read-mostly: the only mutating verbs
//! are pause, abort, threshold edits, and operator-initiated rollback. This
//! module ships the wire types so the Axum server (`gaussclaw-surfaces`) and
//! the Ratatui dashboard (`gaussclaw-tui`) render the same
//! [`crate::event::CycleEvent`] stream and "can never disagree about cycle
//! state".

use serde::{Deserialize, Serialize};

/// A routed, retrieval-grounded answer with attribution (Appendix E
/// `POST /v1/query` response `Answer`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Answer {
    /// The answer text.
    pub text: String,
    /// Per-expert attribution `(model slug, fusion weight)`.
    pub experts: Vec<ExpertAttribution>,
    /// Retrieved provenance sources cited in the answer.
    pub sources: Vec<String>,
    /// Total cost of the query in USD.
    pub cost_usd: f64,
}

/// One expert's attribution in an answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExpertAttribution {
    /// OpenRouter model slug.
    pub model: String,
    /// Fusion weight `wᵢ`.
    pub weight: f64,
}

/// Live loop status (Appendix E `GET /v1/rsi/cycles`): the dashboard's headline
/// numbers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CycleStatus {
    /// Current cycle `t`.
    pub t: u32,
    /// Budget `Bmax`.
    pub budget: u32,
    /// Online productivity estimate `ρ̂`.
    pub rho_hat: f64,
    /// Current combined GDI.
    pub gdi: f64,
    /// Drift threshold `τ`.
    pub tau: f64,
    /// Remaining-cycle forecast `T(ε)` (Eq. 8), `None` if not contracting.
    pub forecast: Option<u32>,
    /// Live synergy count (Theorem 2(b)).
    pub synergy_count: u64,
}

/// The TUI dashboard panels (paper Appendix D, Table V).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TuiPanel {
    /// Header: `t/Bmax`, state, `ε`, `τ`.
    Header,
    /// Admission sparkline: `µ(Aₜ)`, `ρ̂`, `T(ε)`.
    Admission,
    /// Drift gauges: the four GDI components vs. `τ`.
    Drift,
    /// Router table: arm, pulls, `θ̂ᵀφ`, UCB, cost.
    Router,
    /// Verifier bar chart: queue depth, pass/fail/abstain by tier.
    Verifier,
    /// Log tail.
    Log,
}

impl TuiPanel {
    /// All panels in dashboard layout order (Listing 8).
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::Header,
            Self::Admission,
            Self::Drift,
            Self::Router,
            Self::Verifier,
            Self::Log,
        ]
    }

    /// The Ratatui widget kind backing this panel (Table V).
    #[must_use]
    pub const fn widget(self) -> &'static str {
        match self {
            Self::Header => "Paragraph",
            Self::Admission => "Sparkline",
            Self::Drift => "Gauge",
            Self::Router => "Table",
            Self::Verifier => "BarChart",
            Self::Log => "List",
        }
    }
}

/// The read-mostly mutating control verbs the UI exposes (paper §IV.H): the
/// only human-in-the-loop control points the safety analysis assumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ControlVerb {
    /// Pause the loop.
    Pause,
    /// Resume a paused loop.
    Resume,
    /// Abort the loop.
    Abort,
    /// Edit the convergence/drift thresholds.
    EditThresholds {
        /// New tolerance `ε`.
        eps: f64,
        /// New drift threshold `τ`.
        tau: f64,
        /// New patience `k`.
        patience: u32,
    },
    /// Operator-initiated rollback to a snapshot (confirmed).
    Rollback {
        /// Target snapshot cycle index.
        to: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_panels_are_listed() {
        assert_eq!(TuiPanel::all().len(), 6);
        assert_eq!(TuiPanel::Admission.widget(), "Sparkline");
    }

    #[test]
    fn answer_round_trips_through_serde() {
        let a = Answer {
            text: "42".into(),
            experts: vec![ExpertAttribution {
                model: "openai/gpt-4o".into(),
                weight: 0.6,
            }],
            sources: vec!["claim:1".into()],
            cost_usd: 0.02,
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Answer = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn control_verbs_tag_by_verb() {
        let json = serde_json::to_string(&ControlVerb::Rollback { to: 11 }).unwrap();
        assert!(json.contains("\"verb\":\"rollback\""), "{json}");
    }

    #[test]
    fn cycle_status_carries_the_forecast() {
        let s = CycleStatus {
            t: 12,
            budget: 50,
            rho_hat: 0.18,
            gdi: 0.31,
            tau: 0.44,
            forecast: Some(31),
            synergy_count: 47,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: CycleStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back.forecast, Some(31));
    }
}
