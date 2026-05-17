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
//! Outbound is queued in an in-memory outbox; the surface plane drains
//! it and POSTs to `https://api.telegram.org/bot<token>/sendMessage`.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::KernelHandle;
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
        self.outbox.lock().await.push(msg);
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
