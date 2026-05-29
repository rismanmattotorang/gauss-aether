//! `gaussclaw-api-modes` — OpenAI-compatible API-mode shims.
//!
//! Phase 4 of the GaussClaw Hermes-to-Rust port. This crate owns the
//! wire shapes and engine conversions for the OpenAI-compatible
//! surface (`/v1/chat/completions`, `/v1/models`) so an unmodified
//! OpenAI SDK pointed at a running `gaussclaw serve` keeps working.
//!
//! It is deliberately framework-agnostic: the axum handlers that mount
//! these routes live in `gaussclaw-web` (which owns the agent loop and
//! server state). This crate is pure types + total conversion
//! functions, fully unit-testable without a server.
//!
//! See [`openai`] for the request/response shapes and the
//! [`prompt_from_request`] / [`response_from_completion`] /
//! [`model_list`] conversions.
#![allow(clippy::doc_markdown)]

pub mod openai;

pub use openai::{
    map_finish_reason, model_list, prompt_from_request, response_from_completion, stream_chunks,
    ApiError, ApiErrorBody, ChatChoice, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, ChatMessage, ChunkChoice, Delta, ModelCard, ModelList, RequestError,
    Usage,
};
