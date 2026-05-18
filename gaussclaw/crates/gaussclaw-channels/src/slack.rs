//! [`SlackChannel`] — Slack adapter with `v0=` signed webhook ingress.
//!
//! Slack signs every webhook with HMAC-SHA256, encoded as
//! `v0=<hex>` and accompanied by an `X-Slack-Request-Timestamp` header.
//! The signed-string base is the literal `"v0:<ts>:<body>"`. This
//! adapter:
//!
//! 1. Verifies the timestamp is within ± 5 minutes of now (replay-attack
//!    guard).
//! 2. Verifies the HMAC in constant time via the canonical
//!    [`crate::hmac_verify`] primitive.
//! 3. Builds a typed [`ChannelMessage`] with the operator-chosen taint.
//!
//! Outbound is queued in an in-memory outbox; the transport layer
//! drains it and POSTs to the Slack webhook URL. Wiring the live HTTP
//! transport is the responsibility of the `gaussclaw-surfaces`
//! gateway plane; the typed adapter is the surface contract.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::KernelHandle;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{
    hex_decode, ChannelError, ChannelMessage, ChannelResult, ChannelTrait, OutboundMessage,
    SecretStore,
};

/// Slack webhook adapter.
pub struct SlackChannel {
    id: String,
    signing_secret_handle: String,
    secrets: Arc<dyn SecretStore>,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    /// Allowable clock skew between Slack and the receiver, in seconds.
    max_skew_secs: i64,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl SlackChannel {
    /// Build a Slack adapter. The signing secret is resolved through the
    /// supplied [`SecretStore`] (defaults to env-var lookup under the
    /// `SLACK_SIGNING_SECRET` handle).
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        kernel: KernelHandle,
        signing_secret_handle: impl Into<String>,
    ) -> Self {
        Self {
            id: "slack".into(),
            signing_secret_handle: signing_secret_handle.into(),
            secrets,
            kernel,
            default_taint: TaintLabel::Web,
            max_skew_secs: 5 * 60,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Override the maximum allowed timestamp skew (default ± 5 minutes).
    #[must_use]
    pub const fn with_max_skew_secs(mut self, secs: i64) -> Self {
        self.max_skew_secs = secs;
        self
    }

    /// Verify a Slack `v0=` signed webhook and build a typed message.
    ///
    /// # Errors
    /// Returns [`ChannelError::BadSignature`] on signature or timestamp
    /// failure, [`ChannelError::MissingSecret`] if the signing-secret
    /// handle is unresolvable, or [`ChannelError::Denied`] if the kernel
    /// admit gate refuses.
    pub async fn handle_webhook(
        &self,
        timestamp_header: &str,
        signature_header: &str,
        raw_body: &[u8],
        sender: impl Into<String>,
    ) -> ChannelResult<ChannelMessage> {
        let secret = self
            .secrets
            .get(&self.signing_secret_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.signing_secret_handle.clone()))?;

        // 1. timestamp freshness
        let ts: i64 = timestamp_header
            .parse()
            .map_err(|_| ChannelError::BadSignature)?;
        let now = unix_now()?;
        if (now - ts).abs() > self.max_skew_secs {
            return Err(ChannelError::BadSignature);
        }

        // 2. signature
        let candidate = hex_decode(signature_header.trim_start_matches("v0="))
            .map_err(|()| ChannelError::BadSignature)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret).map_err(|_| ChannelError::BadSignature)?;
        mac.update(b"v0:");
        mac.update(timestamp_header.as_bytes());
        mac.update(b":");
        mac.update(raw_body);
        let expected = mac.finalize().into_bytes();
        if !bool::from(expected.as_slice().ct_eq(&candidate)) {
            return Err(ChannelError::BadSignature);
        }

        // 3. kernel admit
        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;

        let body = std::str::from_utf8(raw_body).unwrap_or("").to_string();
        Ok(ChannelMessage::new(&self.id, sender, body).with_taint(self.default_taint))
    }

    /// Drain queued outbound messages.
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }
}

#[async_trait]
impl ChannelTrait for SlackChannel {
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
        // The HTTP transport lives in `gaussclaw-surfaces`; the adapter
        // queues structured outbound to be drained there.
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

fn unix_now() -> ChannelResult<i64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| ChannelError::Transport(format!("clock: {e}")))
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

    fn sign(secret: &[u8], ts: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(b"v0:");
        mac.update(ts.as_bytes());
        mac.update(b":");
        mac.update(body);
        let bytes = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(2 * bytes.len());
        for b in &bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        format!("v0={hex}")
    }

    #[tokio::test]
    async fn accepts_correctly_signed_recent_webhook() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("SLACK_SIGNING_SECRET", b"shhh".to_vec());
        let ch = SlackChannel::new(secrets, kernel(), "SLACK_SIGNING_SECRET");
        let ts = unix_now().unwrap().to_string();
        let body = br#"{"event":"hello"}"#;
        let sig = sign(b"shhh", &ts, body);
        let msg = ch
            .handle_webhook(&ts, &sig, body, "@alice")
            .await
            .expect("verify");
        assert_eq!(msg.channel, "slack");
        assert_eq!(msg.taint, TaintLabel::Web);
        assert_eq!(msg.sender, "@alice");
    }

    #[tokio::test]
    async fn rejects_bad_signature() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("SLACK_SIGNING_SECRET", b"shhh".to_vec());
        let ch = SlackChannel::new(secrets, kernel(), "SLACK_SIGNING_SECRET");
        let ts = unix_now().unwrap().to_string();
        let result = ch.handle_webhook(&ts, "v0=deadbeef", b"body", "@bob").await;
        assert!(matches!(result, Err(ChannelError::BadSignature)));
    }

    #[tokio::test]
    async fn rejects_stale_timestamp() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("SLACK_SIGNING_SECRET", b"shhh".to_vec());
        let ch = SlackChannel::new(secrets, kernel(), "SLACK_SIGNING_SECRET");
        let stale_ts = (unix_now().unwrap() - 3600).to_string();
        let body = br#"{}"#;
        let sig = sign(b"shhh", &stale_ts, body);
        let result = ch.handle_webhook(&stale_ts, &sig, body, "@c").await;
        assert!(matches!(result, Err(ChannelError::BadSignature)));
    }

    #[tokio::test]
    async fn outbound_queues_to_outbox() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let ch = SlackChannel::new(secrets, kernel(), "h");
        ch.send(OutboundMessage::new("#general", "hi"))
            .await
            .unwrap();
        let out = ch.drain_outbox().await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body, "hi");
    }
}
