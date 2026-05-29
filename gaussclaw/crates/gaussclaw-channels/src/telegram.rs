//! [`TelegramChannel`] — Telegram Bot API adapter.
//!
//! Telegram offers two ingress modes:
//!
//! 1. **Webhook** — Telegram POSTs updates to a URL you register, with
//!    the `X-Telegram-Bot-Api-Secret-Token` header set to a value you
//!    chose at `setWebhook` time. The adapter verifies the header via
//!    constant-time comparison and accepts the update.
//! 2. **Long-poll** — the bot calls `getUpdates` itself. This adapter
//!    exposes the typed wire shape; the live transport lives in the
//!    surface plane.
//!
//! Outbound delivery: with an [`HttpClient`] transport configured,
//! [`TelegramChannel::send`] POSTs to
//! `https://api.telegram.org/bot<token>/sendMessage`. Without one it
//! buffers to an in-memory outbox.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::KernelHandle;
use gaussclaw_tools::{HttpClient, HttpMethod, HttpRequest};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{
    ChannelError, ChannelMessage, ChannelResult, ChannelTrait, OutboundMessage, SecretStore,
};

/// Telegram bot adapter.
pub struct TelegramChannel {
    id: String,
    token_handle: String,
    webhook_secret_handle: Option<String>,
    secrets: Arc<dyn SecretStore>,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    /// Optional outbound HTTP transport. `None` → buffer to `outbox`.
    http: Option<Arc<dyn HttpClient>>,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl TelegramChannel {
    /// Build a Telegram adapter. `token_handle` is the secret-store key
    /// where the bot token lives (typically `TELEGRAM_BOT_TOKEN`).
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        kernel: KernelHandle,
        token_handle: impl Into<String>,
    ) -> Self {
        Self {
            id: "telegram".into(),
            token_handle: token_handle.into(),
            webhook_secret_handle: None,
            secrets,
            kernel,
            default_taint: TaintLabel::Web,
            http: None,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Configure the webhook header secret, enabling
    /// `X-Telegram-Bot-Api-Secret-Token` verification on inbound.
    #[must_use]
    pub fn with_webhook_secret(mut self, handle: impl Into<String>) -> Self {
        self.webhook_secret_handle = Some(handle.into());
        self
    }

    /// Attach an HTTP transport so [`Self::send`] delivers via the
    /// Telegram Bot API instead of buffering.
    #[must_use]
    pub fn with_http(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = Some(http);
        self
    }

    /// Build the `sendMessage` request for `msg` under `token`. Pure, so
    /// the wire shape is unit-testable. `recipient` is the Telegram
    /// chat id (sent as a number when it parses as one, else a string,
    /// covering both numeric ids and `@channelusername`).
    #[must_use]
    pub fn build_send_request(token: &str, msg: &OutboundMessage) -> HttpRequest {
        let mut headers = BTreeMap::new();
        headers.insert(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        );
        let chat_id = msg.recipient.parse::<i64>().map_or_else(
            |_| serde_json::Value::String(msg.recipient.clone()),
            |n| serde_json::Value::from(n),
        );
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": msg.body,
        })
        .to_string();
        HttpRequest::new(
            HttpMethod::Post,
            format!("https://api.telegram.org/bot{token}/sendMessage"),
            headers,
            Some(body),
        )
    }

    /// Verify the optional webhook secret header and turn a Telegram
    /// `Update` payload into a typed [`ChannelMessage`].
    ///
    /// # Errors
    /// [`ChannelError::BadSignature`] on header mismatch,
    /// [`ChannelError::MissingSecret`] if a configured webhook secret is
    /// unresolvable, [`ChannelError::Transport`] on JSON parse failure,
    /// [`ChannelError::Denied`] on kernel-admit refusal.
    pub async fn handle_webhook(
        &self,
        secret_header: Option<&str>,
        raw_body: &[u8],
    ) -> ChannelResult<ChannelMessage> {
        // Optional but recommended webhook-secret check.
        if let Some(handle) = self.webhook_secret_handle.as_ref() {
            let expected = self
                .secrets
                .get(handle)
                .ok_or_else(|| ChannelError::MissingSecret(handle.clone()))?;
            let provided = secret_header.unwrap_or("").as_bytes();
            if !bool::from(expected.as_slice().ct_eq(provided)) {
                return Err(ChannelError::BadSignature);
            }
        }

        let update: TelegramUpdate = serde_json::from_slice(raw_body)
            .map_err(|e| ChannelError::Transport(format!("telegram parse: {e}")))?;
        let (sender, body, chat_id) = update.extract()?;

        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;

        Ok(ChannelMessage::new(&self.id, sender, body)
            .with_taint(self.default_taint)
            .with_meta("chat_id", serde_json::json!(chat_id)))
    }

    /// Outbox accessor for tests / a transport driver.
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }

    /// Resolve the bot token at send time. Useful for the transport
    /// driver to build the URL `https://api.telegram.org/bot<token>/…`.
    #[must_use]
    pub fn token(&self) -> Option<Vec<u8>> {
        self.secrets.get(&self.token_handle)
    }
}

#[async_trait]
impl ChannelTrait for TelegramChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_GET
    }

    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        let Some(http) = &self.http else {
            self.outbox.lock().await.push(msg);
            return Ok(());
        };
        let token = self
            .secrets
            .get(&self.token_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.token_handle.clone()))?;
        let token = String::from_utf8(token)
            .map_err(|_| ChannelError::Transport("bot token is not valid UTF-8".into()))?;
        let req = Self::build_send_request(&token, &msg);
        let resp = http
            .request(req)
            .await
            .map_err(|e| ChannelError::Transport(format!("telegram send: {e}")))?;
        if !(200..300).contains(&resp.status) {
            return Err(ChannelError::Transport(format!(
                "telegram sendMessage HTTP {}: {}",
                resp.status, resp.body
            )));
        }
        // Telegram replies `{"ok": false, "description": "..."}` on logical
        // failures even with a 200; surface those.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp.body) {
            if v.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                let desc = v
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                return Err(ChannelError::Transport(format!(
                    "telegram sendMessage error: {desc}"
                )));
            }
        }
        Ok(())
    }
}

/// Sub-set of the Telegram `Update` schema we need to extract a typed
/// message. Anything else is preserved in the message metadata by the
/// transport driver, but the adapter's contract is the user-visible
/// (`sender`, `body`, `chat_id`) triple.
#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    #[serde(default)]
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    chat: TelegramChat,
    from: Option<TelegramUser>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    username: Option<String>,
    first_name: Option<String>,
}

impl TelegramUpdate {
    fn extract(self) -> ChannelResult<(String, String, i64)> {
        let msg = self
            .message
            .ok_or_else(|| ChannelError::Transport("telegram update missing `message`".into()))?;
        let sender = msg
            .from
            .and_then(|u| u.username.or(u.first_name))
            .unwrap_or_else(|| format!("update:{}", self.update_id));
        let body = msg.text.unwrap_or_default();
        Ok((sender, body, msg.chat.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemorySecretStore;
    use gauss_kernel::PrivilegedKernel;
    use gaussclaw_agent::KernelHandle;
    use std::sync::Arc;

    fn kernel() -> KernelHandle {
        KernelHandle::new(Arc::new(PrivilegedKernel::new(CapToken::TOP)))
    }

    #[tokio::test]
    async fn accepts_update_without_webhook_secret() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let ch = TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN");
        let body = br#"{"update_id":42,"message":{"chat":{"id":99},"from":{"username":"alice"},"text":"hi"}}"#;
        let msg = ch.handle_webhook(None, body).await.expect("ok");
        assert_eq!(msg.sender, "alice");
        assert_eq!(msg.body, "hi");
        assert_eq!(msg.metadata["chat_id"], 99);
    }

    #[test]
    fn build_send_request_targets_sendmessage_with_numeric_chat_id() {
        let msg = OutboundMessage::new("12345", "hello world");
        let req = TelegramChannel::build_send_request("TOK", &msg);
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.telegram.org/botTOK/sendMessage");
        let body: serde_json::Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
        // Numeric recipients become a JSON number; text is carried verbatim.
        assert_eq!(body["chat_id"], 12345);
        assert_eq!(body["text"], "hello world");
    }

    #[test]
    fn build_send_request_keeps_username_recipient_as_string() {
        let msg = OutboundMessage::new("@channel", "yo");
        let req = TelegramChannel::build_send_request("TOK", &msg);
        let body: serde_json::Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["chat_id"], "@channel");
    }

    #[tokio::test]
    async fn send_delivers_over_http_when_configured() {
        use gaussclaw_tools::{HttpMethod, HttpResponse, MockHttpClient};
        use std::collections::BTreeMap;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("TELEGRAM_BOT_TOKEN", b"BOTTOKEN".to_vec());
        let mock = Arc::new(MockHttpClient::new());
        mock.expect(
            HttpMethod::Post,
            "https://api.telegram.org/botBOTTOKEN/sendMessage",
            HttpResponse::new(200, BTreeMap::new(), r#"{"ok":true}"#.into(), false),
        );
        let ch = TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN")
            .with_http(mock.clone());
        ch.send(OutboundMessage::new("777", "ping")).await.unwrap();

        let calls = mock.observed();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://api.telegram.org/botBOTTOKEN/sendMessage");
        // Nothing buffered — it went over the wire.
        assert!(ch.drain_outbox().await.is_empty());
    }

    #[tokio::test]
    async fn send_surfaces_logical_failure_from_ok_false() {
        use gaussclaw_tools::{HttpMethod, HttpResponse, MockHttpClient};
        use std::collections::BTreeMap;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("TELEGRAM_BOT_TOKEN", b"T".to_vec());
        let mock = Arc::new(MockHttpClient::new());
        mock.expect(
            HttpMethod::Post,
            "https://api.telegram.org/botT/sendMessage",
            HttpResponse::new(
                200,
                BTreeMap::new(),
                r#"{"ok":false,"description":"chat not found"}"#.into(),
                false,
            ),
        );
        let ch =
            TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN").with_http(mock);
        let err = ch.send(OutboundMessage::new("1", "x")).await.unwrap_err();
        assert!(matches!(err, ChannelError::Transport(m) if m.contains("chat not found")));
    }

    #[tokio::test]
    async fn send_without_http_buffers_to_outbox() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let ch = TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN");
        ch.send(OutboundMessage::new("1", "buffered")).await.unwrap();
        let drained = ch.drain_outbox().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].body, "buffered");
    }

    #[tokio::test]
    async fn rejects_missing_webhook_secret_when_configured() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("TG_WEBHOOK_SECRET", b"hush".to_vec());
        let ch = TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN")
            .with_webhook_secret("TG_WEBHOOK_SECRET");
        let body = br#"{"update_id":1,"message":{"chat":{"id":1},"text":"x"}}"#;
        let result = ch.handle_webhook(Some("wrong"), body).await;
        assert!(matches!(result, Err(ChannelError::BadSignature)));
    }

    #[tokio::test]
    async fn accepts_correct_webhook_secret() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("TG_WEBHOOK_SECRET", b"hush".to_vec());
        let ch = TelegramChannel::new(secrets, kernel(), "TELEGRAM_BOT_TOKEN")
            .with_webhook_secret("TG_WEBHOOK_SECRET");
        let body = br#"{"update_id":1,"message":{"chat":{"id":2},"from":{"first_name":"Bob"},"text":"hello"}}"#;
        let msg = ch.handle_webhook(Some("hush"), body).await.expect("ok");
        assert_eq!(msg.sender, "Bob");
        assert_eq!(msg.body, "hello");
    }

    #[tokio::test]
    async fn rejects_malformed_payload() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let ch = TelegramChannel::new(secrets, kernel(), "T");
        let result = ch.handle_webhook(None, b"not json").await;
        assert!(matches!(result, Err(ChannelError::Transport(_))));
    }
}
