//! `gaussclaw gateway` — the messaging gateway server.
//!
//! Fans inbound vendor webhooks (Slack, Discord, Telegram) into the
//! native channel adapters, dispatches each verified message through the
//! agent, and delivers the reply back over the originating channel.
//!
//! Each route:
//! 1. extracts the vendor's signature/timestamp headers + raw body,
//! 2. calls the adapter's `handle_webhook` (constant-time signature
//!    verification + typed parse),
//! 3. on success runs one agent turn over the message text, and
//! 4. sends the assistant reply to the originating channel/chat via the
//!    adapter's HTTP transport.
//!
//! Verification failures map to `401`; a missing server secret to `500`;
//! a kernel-admit denial to `403`. Webhook acceptance (`200`) does not
//! block on reply delivery — a failed outbound send is logged, not
//! surfaced to the vendor (which would otherwise retry the delivery).

// These items are `pub(crate)` because `main.rs` (the crate root, a
// sibling of this private module) builds the state and router.
// `unreachable_pub` forbids plain `pub` here, while `redundant_pub_crate`
// flags the `pub(crate)` as redundant in a binary — the two lints
// conflict, so we silence the latter with that rationale.
#![allow(clippy::redundant_pub_crate)]

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use gauss_core::TaintLabel;
use gaussclaw_agent::{AgentLoop, Message, Prompt};
use gaussclaw_channels::{
    ChannelError, ChannelMessage, ChannelTrait, DiscordChannel, OutboundMessage, SlackChannel,
    TelegramChannel,
};

/// Shared state for the gateway routes.
#[derive(Clone)]
pub(crate) struct GatewayState {
    /// Agent loop driving replies (its `policy` runs a single turn).
    pub(crate) agent: Arc<AgentLoop>,
    /// Model id sent to the provider codec on each reply turn.
    pub(crate) model: String,
    pub(crate) slack: Arc<SlackChannel>,
    pub(crate) discord: Arc<DiscordChannel>,
    pub(crate) telegram: Arc<TelegramChannel>,
}

/// Build the gateway router. Separated from the server bind so it's
/// unit-testable via `tower::ServiceExt::oneshot`.
pub(crate) fn gateway_router(state: GatewayState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/webhooks/slack", post(slack_webhook))
        .route("/webhooks/discord", post(discord_webhook))
        .route("/webhooks/telegram", post(telegram_webhook))
        .with_state(state)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// Map a verification/parse error to an HTTP status.
const fn verify_status(e: &ChannelError) -> StatusCode {
    match e {
        ChannelError::BadSignature
        | ChannelError::SignatureInvalid(_)
        | ChannelError::ReplayWindow => StatusCode::UNAUTHORIZED,
        ChannelError::Denied(_) => StatusCode::FORBIDDEN,
        ChannelError::MissingSecret(_) => StatusCode::INTERNAL_SERVER_ERROR,
        // A parse failure on an otherwise-authenticated body is a client
        // error.
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Extract the reply target from the parsed message's metadata, as a
/// string (numbers — e.g. Telegram chat ids — are stringified).
fn reply_target(msg: &ChannelMessage, key: &str) -> Option<String> {
    msg.metadata.get(key).map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// Run one agent turn over `msg.body` and deliver the reply to
/// `recipient` via `adapter`. Best-effort: delivery failures are logged.
async fn dispatch_and_reply(
    state: &GatewayState,
    adapter: Arc<dyn ChannelTrait>,
    msg: &ChannelMessage,
    recipient: String,
) {
    let prompt = Prompt::new(
        state.model.clone(),
        vec![Message::new("user", msg.body.clone())],
    );
    match state.agent.policy.run(prompt, TaintLabel::Web).await {
        std::result::Result::Ok(completion) => {
            if let std::result::Result::Err(e) = adapter
                .send(OutboundMessage::new(recipient, completion.text))
                .await
            {
                tracing::warn!(target: "gaussclaw_bin::gateway", "reply delivery failed: {e}");
            }
        }
        std::result::Result::Err(e) => {
            tracing::warn!(target: "gaussclaw_bin::gateway", "agent turn failed: {e:?}");
        }
    }
}

#[axum::debug_handler]
async fn slack_webhook(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let ts = header(&headers, "x-slack-request-timestamp").to_string();
    let sig = header(&headers, "x-slack-signature").to_string();
    match state.slack.handle_webhook(&ts, &sig, &body, "slack").await {
        std::result::Result::Ok(msg) => {
            if let Some(recipient) = reply_target(&msg, "channel") {
                dispatch_and_reply(&state, state.slack.clone(), &msg, recipient).await;
            }
            StatusCode::OK
        }
        std::result::Result::Err(e) => verify_status(&e),
    }
}

#[axum::debug_handler]
async fn discord_webhook(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let ts = header(&headers, "x-signature-timestamp").to_string();
    let sig = header(&headers, "x-signature-ed25519").to_string();
    match state.discord.handle_webhook(&ts, &sig, &body).await {
        std::result::Result::Ok(msg) => {
            if let Some(recipient) = reply_target(&msg, "channel_id") {
                dispatch_and_reply(&state, state.discord.clone(), &msg, recipient).await;
            }
            StatusCode::OK
        }
        std::result::Result::Err(e) => verify_status(&e),
    }
}

#[axum::debug_handler]
async fn telegram_webhook(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let secret = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok());
    match state.telegram.handle_webhook(secret, &body).await {
        std::result::Result::Ok(msg) => {
            if let Some(recipient) = reply_target(&msg, "chat_id") {
                dispatch_and_reply(&state, state.telegram.clone(), &msg, recipient).await;
            }
            StatusCode::OK
        }
        std::result::Result::Err(e) => verify_status(&e),
    }
}
