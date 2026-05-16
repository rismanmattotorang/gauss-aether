//! `gauss-gateway` — wire types for the REST / WS / SSE gateway
//! (paper §XIII.D).
//!
//! Phase 9 ships the stable wire-shape only — the actual `axum`-backed
//! server lives in a feature-gated additive deployment (the
//! `gauss-server` workspace crate at Phase 11). The split lets plugin
//! authors take a dep on this crate to produce / consume gateway
//! payloads without pulling in the HTTP machinery.
//!
//! Surfaces covered:
//!
//! * [`TurnRequest`] / [`TurnResponse`] — `POST /v1/turn` shape.
//! * [`HealthResponse`] — `GET /v1/health` shape (`gauss-health`
//!   serialised wire).
//! * [`StreamEvent`] — server-sent events.
//! * [`OpenAiChatRequest`] / [`OpenAiChatResponse`] — OpenAI-compatible
//!   proxy shapes per SPECS §XIII.D, including the `messages` /
//!   `choices` / `usage` triad.

pub mod openai;
pub mod stream;
pub mod turn;

pub use openai::{
    OpenAiChatChoice, OpenAiChatMessage, OpenAiChatRequest, OpenAiChatResponse, OpenAiUsage,
};
pub use stream::StreamEvent;
pub use turn::{HealthResponse, TurnRequest, TurnResponse};
