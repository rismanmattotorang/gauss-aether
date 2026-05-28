//! Outbox transport — the layer that actually POSTs queued
//! [`OutboundMessage`]s to the upstream provider.
//!
//! Sprint 4 of "Wire the Loop". Each channel adapter
//! ([`crate::SlackChannel`], [`crate::DiscordChannel`],
//! [`crate::TelegramChannel`]) already buffers outbound traffic in an
//! internal `Vec<OutboundMessage>`; this module ships the missing wire
//! between that buffer and the upstream HTTP API.
//!
//! ## Design
//!
//! - One trait, [`OutboxTransport`], parameterised over a single
//!   `OutboundMessage`. Each adapter has its own implementation that
//!   knows the upstream's URL format and auth shape.
//! - Every implementation depends only on
//!   [`gaussclaw_tools::HttpClient`] — no direct `reqwest` use here.
//!   Bins inject `gaussclaw_http::ReqwestHttpClient` in production;
//!   tests inject [`gaussclaw_tools::MockHttpClient`].
//! - Transports never retry. Backoff / circuit-breaking is owned by
//!   the gateway daemon a layer above (Sprint 4 §2 follow-on).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_tools::{HttpClient, HttpMethod, HttpRequest};
use serde_json::json;

use crate::{ChannelError, ChannelResult, OutboundMessage};

/// Outbound transport for a single channel.
#[async_trait]
pub trait OutboxTransport: Send + Sync {
    /// Channel id this transport handles (matches
    /// [`crate::ChannelTrait::id`]). Used by the gateway daemon to
    /// route a drained outbox to the right transport.
    fn channel_id(&self) -> &str;

    /// POST one message to the upstream. Implementations must not
    /// retry — the caller decides whether to back off and re-queue.
    async fn deliver(&self, msg: &OutboundMessage) -> ChannelResult<()>;

    /// Drain a batch of messages and deliver each in order. The default
    /// implementation walks the batch and stops on the first failure,
    /// returning the index of the failing message so the caller can
    /// re-queue the tail.
    async fn deliver_batch(
        &self,
        batch: &[OutboundMessage],
    ) -> Result<(), (usize, ChannelError)> {
        for (i, msg) in batch.iter().enumerate() {
            if let Err(e) = self.deliver(msg).await {
                return Err((i, e));
            }
        }
        Ok(())
    }
}

// ─── Slack ─────────────────────────────────────────────────────────────────

/// Slack outbox transport. POSTs to a webhook URL (Incoming Webhook
/// or chat.postMessage). The URL is opaque to the transport; secret
/// management is the gateway's responsibility.
pub struct SlackOutbox {
    client: Arc<dyn HttpClient>,
    webhook_url: String,
}

impl SlackOutbox {
    /// Build a Slack outbox bound to `webhook_url`.
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>, webhook_url: impl Into<String>) -> Self {
        Self {
            client,
            webhook_url: webhook_url.into(),
        }
    }
}

#[async_trait]
impl OutboxTransport for SlackOutbox {
    fn channel_id(&self) -> &str {
        "slack"
    }

    async fn deliver(&self, msg: &OutboundMessage) -> ChannelResult<()> {
        let mut body = json!({ "text": msg.body });
        if !msg.recipient.is_empty() {
            body["channel"] = serde_json::Value::String(msg.recipient.clone());
        }
        post_json(&self.client, &self.webhook_url, &body).await
    }
}

// ─── Discord ──────────────────────────────────────────────────────────────

/// Discord outbox transport. POSTs to a webhook URL.
pub struct DiscordOutbox {
    client: Arc<dyn HttpClient>,
    webhook_url: String,
}

impl DiscordOutbox {
    /// Build a Discord outbox bound to `webhook_url`.
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>, webhook_url: impl Into<String>) -> Self {
        Self {
            client,
            webhook_url: webhook_url.into(),
        }
    }
}

#[async_trait]
impl OutboxTransport for DiscordOutbox {
    fn channel_id(&self) -> &str {
        "discord"
    }

    async fn deliver(&self, msg: &OutboundMessage) -> ChannelResult<()> {
        let mut body = json!({ "content": msg.body });
        if !msg.recipient.is_empty() {
            body["embeds"] = json!([{ "author": { "name": msg.recipient } }]);
        }
        post_json(&self.client, &self.webhook_url, &body).await
    }
}

// ─── Telegram ─────────────────────────────────────────────────────────────

/// Telegram bot-API outbox transport. POSTs to
/// `https://api.telegram.org/bot<token>/sendMessage` with
/// `chat_id` = recipient.
pub struct TelegramOutbox {
    client: Arc<dyn HttpClient>,
    bot_token: String,
    api_base: String,
}

impl TelegramOutbox {
    /// Build a Telegram outbox bound to `bot_token`.
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>, bot_token: impl Into<String>) -> Self {
        Self {
            client,
            bot_token: bot_token.into(),
            api_base: "https://api.telegram.org".into(),
        }
    }

    /// Override the API base (for tests against a mock server).
    #[must_use]
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }
}

#[async_trait]
impl OutboxTransport for TelegramOutbox {
    fn channel_id(&self) -> &str {
        "telegram"
    }

    async fn deliver(&self, msg: &OutboundMessage) -> ChannelResult<()> {
        if msg.recipient.is_empty() {
            return Err(ChannelError::Transport(
                "telegram outbound requires a non-empty recipient (chat_id)".into(),
            ));
        }
        let url = format!(
            "{base}/bot{token}/sendMessage",
            base = self.api_base.trim_end_matches('/'),
            token = self.bot_token,
        );
        let body = json!({ "chat_id": msg.recipient, "text": msg.body });
        post_json(&self.client, &url, &body).await
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────

async fn post_json(
    client: &Arc<dyn HttpClient>,
    url: &str,
    body: &serde_json::Value,
) -> ChannelResult<()> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    let req = HttpRequest::new(
        HttpMethod::Post,
        url.to_string(),
        headers,
        Some(body.to_string()),
    );
    let resp = client
        .request(req)
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))?;
    if !(200..300).contains(&resp.status) {
        let snippet = resp.body.chars().take(512).collect::<String>();
        return Err(ChannelError::Transport(format!(
            "upstream {status}: {snippet}",
            status = resp.status
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_tools::{HttpResponse, MockHttpClient};

    fn ok_response() -> HttpResponse {
        HttpResponse::new(200, BTreeMap::new(), "{}".to_string(), false)
    }

    fn err_response() -> HttpResponse {
        HttpResponse::new(
            500,
            BTreeMap::new(),
            "upstream exploded".to_string(),
            false,
        )
    }

    #[tokio::test]
    async fn slack_outbox_posts_text_payload() {
        let mock = MockHttpClient::new();
        let url = "https://hooks.slack.test/services/T/B/X";
        mock.expect(HttpMethod::Post, url, ok_response());
        let client: Arc<dyn HttpClient> = Arc::new(mock.clone());
        let outbox = SlackOutbox::new(client, url);
        let msg = OutboundMessage::new("#general", "hello world");
        outbox.deliver(&msg).await.expect("ok");
        let seen = mock.observed();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, url);
        let body: serde_json::Value =
            serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["text"], "hello world");
        assert_eq!(body["channel"], "#general");
    }

    #[tokio::test]
    async fn discord_outbox_posts_content_payload() {
        let mock = MockHttpClient::new();
        let url = "https://discord.test/api/webhooks/x/y";
        mock.expect(HttpMethod::Post, url, ok_response());
        let client: Arc<dyn HttpClient> = Arc::new(mock.clone());
        let outbox = DiscordOutbox::new(client, url);
        let msg = OutboundMessage::new("", "hello");
        outbox.deliver(&msg).await.expect("ok");
        let seen = mock.observed();
        let body: serde_json::Value =
            serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["content"], "hello");
        assert!(body.get("embeds").is_none());
    }

    #[tokio::test]
    async fn telegram_outbox_targets_send_message_with_chat_id() {
        let mock = MockHttpClient::new();
        let url = "https://tg.test/botBOT_TOKEN_123/sendMessage";
        mock.expect(HttpMethod::Post, url, ok_response());
        let client: Arc<dyn HttpClient> = Arc::new(mock.clone());
        let outbox =
            TelegramOutbox::new(client, "BOT_TOKEN_123").with_api_base("https://tg.test");
        let msg = OutboundMessage::new("42", "hi");
        outbox.deliver(&msg).await.expect("ok");
        let seen = mock.observed();
        assert_eq!(seen[0].url, url);
        let body: serde_json::Value =
            serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["chat_id"], "42");
        assert_eq!(body["text"], "hi");
    }

    #[tokio::test]
    async fn telegram_outbox_rejects_empty_recipient() {
        let mock = MockHttpClient::new();
        let client: Arc<dyn HttpClient> = Arc::new(mock);
        let outbox = TelegramOutbox::new(client, "X").with_api_base("https://tg.test");
        let err = outbox
            .deliver(&OutboundMessage::new("", "hi"))
            .await
            .expect_err("empty chat_id");
        match err {
            ChannelError::Transport(msg) => assert!(msg.contains("chat_id")),
            _ => panic!("expected Transport"),
        }
    }

    #[tokio::test]
    async fn non_2xx_upstream_surfaces_as_transport_error() {
        let mock = MockHttpClient::new();
        let url = "https://hooks.slack.test/";
        mock.expect(HttpMethod::Post, url, err_response());
        let client: Arc<dyn HttpClient> = Arc::new(mock);
        let outbox = SlackOutbox::new(client, url);
        let err = outbox
            .deliver(&OutboundMessage::new("#g", "hi"))
            .await
            .expect_err("500");
        match err {
            ChannelError::Transport(msg) => assert!(msg.contains("500")),
            _ => panic!("expected Transport"),
        }
    }

    #[tokio::test]
    async fn batch_delivery_stops_on_first_failure() {
        // For the same URL the mock returns the same canned response on
        // every call, so we craft three URLs and three different
        // outboxes. Simpler: keep one URL but flip the mock between
        // calls by swapping registered response after the second call.
        // Cleaner: use three URL/outbox pairs and three single-shot
        // batches isn't a true "batch" — instead, point all three at
        // the same URL and rely on the mock returning the canned 200
        // for the first call, then re-register an error response.
        //
        // Cleanest within this mock's contract: register an error for
        // the URL after the first successful call. The mock holds a
        // map keyed by (method, url), so registering twice overwrites.
        let mock = MockHttpClient::new();
        let url = "https://hooks.slack.test/";
        mock.expect(HttpMethod::Post, url, err_response());
        let client: Arc<dyn HttpClient> = Arc::new(mock.clone());
        let outbox = SlackOutbox::new(client, url);
        let batch = vec![
            OutboundMessage::new("#g", "first"),
            OutboundMessage::new("#g", "second"),
        ];
        let result = outbox.deliver_batch(&batch).await;
        match result {
            Err((i, _)) => assert_eq!(i, 0, "should fail at index 0 (first call hits 500)"),
            Ok(()) => panic!("expected failure"),
        }
        // Only one call went out before the batch bailed.
        assert_eq!(mock.observed().len(), 1);
    }
}
