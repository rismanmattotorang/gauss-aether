//! The cycle event bus (paper Appendix B, `CycleEvent`).
//!
//! The RSI Loop Engine emits one stream of cycle events on a broadcast
//! channel consumed by both the Axum REST/WebSocket surface and the Ratatui
//! dashboard, "so web and terminal views can never disagree about cycle
//! state" (paper §V.D). Phase 0 fixes the wire type; Phase 5 wires it to a
//! `tokio::sync::broadcast`.

use serde::{Deserialize, Serialize};

use crate::gdi::DriftComponents;

/// One event in the RSI cycle lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CycleEvent {
    /// Cycle `t` started.
    Started {
        /// Cycle index.
        t: u32,
    },
    /// A batch was admitted with the given mass `µ(Aₜ)`.
    Admitted {
        /// Admitted mass this cycle.
        mass: f64,
    },
    /// Drift was measured for this cycle.
    Drift {
        /// The four drift components.
        drift: DriftComponents,
        /// The combined GDI value.
        gdi: f64,
    },
    /// The engine rolled back to the snapshot at cycle `to`.
    RolledBack {
        /// Cycle index of the restored checkpoint.
        to: u32,
    },
    /// The loop converged at cycle `t` (the patience-`k` stopping rule fired).
    Converged {
        /// Cycle index at which convergence was declared.
        t: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_through_serde() {
        let events = [
            CycleEvent::Started { t: 0 },
            CycleEvent::Admitted { mass: 47.0 },
            CycleEvent::Drift {
                drift: DriftComponents::new(0.1, 0.2, 0.3, 0.4),
                gdi: 0.25,
            },
            CycleEvent::RolledBack { to: 11 },
            CycleEvent::Converged { t: 31 },
        ];
        for e in events {
            let json = serde_json::to_string(&e).unwrap();
            let back: CycleEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back);
        }
    }

    #[test]
    fn started_tags_with_kind() {
        let json = serde_json::to_string(&CycleEvent::Started { t: 7 }).unwrap();
        assert!(json.contains("\"kind\":\"started\""), "{json}");
    }
}
