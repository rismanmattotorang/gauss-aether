//! [`DiscordChannel`] — Discord interactions endpoint adapter.
//!
//! Discord signs every interactions POST with Ed25519, sending the
//! signature in `X-Signature-Ed25519` and the timestamp in
//! `X-Signature-Timestamp`. The signed-string base is the literal
//! concatenation `timestamp || body`. This adapter verifies the
//! signature, admits via the kernel, and emits a typed
//! [`ChannelMessage`].
//!
//! Verification reuses the same Ed25519 verifier path the audit chain
//! already trusts; the public key for the bot lives in the secret
//! store under the operator-chosen handle (typically
//! `DISCORD_PUBLIC_KEY`, stored hex-encoded).

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::KernelHandle;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{
    hex_decode, ChannelError, ChannelMessage, ChannelResult, ChannelTrait, OutboundMessage,
    SecretStore,
};

/// Discord interactions adapter.
pub struct DiscordChannel {
    id: String,
    public_key_handle: String,
    secrets: Arc<dyn SecretStore>,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl DiscordChannel {
    /// Build a Discord adapter. `public_key_handle` is the secret-store
    /// key where the bot's hex-encoded Ed25519 public key lives.
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        kernel: KernelHandle,
        public_key_handle: impl Into<String>,
    ) -> Self {
        Self {
            id: "discord".into(),
            public_key_handle: public_key_handle.into(),
            secrets,
            kernel,
            default_taint: TaintLabel::Web,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify the Ed25519 signature and build a typed message.
    ///
    /// # Errors
    /// [`ChannelError::BadSignature`] on any verification failure,
    /// [`ChannelError::MissingSecret`] if the public key isn't in the
    /// secret store, [`ChannelError::Denied`] on kernel-admit refusal.
    pub async fn handle_webhook(
        &self,
        timestamp_header: &str,
        signature_header: &str,
        raw_body: &[u8],
    ) -> ChannelResult<ChannelMessage> {
        let pk_bytes_hex = self
            .secrets
            .get(&self.public_key_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.public_key_handle.clone()))?;
        let pk_bytes = hex_decode(std::str::from_utf8(&pk_bytes_hex).unwrap_or(""))
            .map_err(|()| ChannelError::BadSignature)?;
        let pk_arr: [u8; 32] = pk_bytes
            .as_slice()
            .try_into()
            .map_err(|_| ChannelError::BadSignature)?;
        let pk = VerifyingKey::from_bytes(&pk_arr).map_err(|_| ChannelError::BadSignature)?;

        let sig_bytes = hex_decode(signature_header).map_err(|()| ChannelError::BadSignature)?;
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| ChannelError::BadSignature)?;
        let sig = Signature::from_bytes(&sig_arr);

        let mut msg = Vec::with_capacity(timestamp_header.len().saturating_add(raw_body.len()));
        msg.extend_from_slice(timestamp_header.as_bytes());
        msg.extend_from_slice(raw_body);
        pk.verify(&msg, &sig).map_err(|_| ChannelError::BadSignature)?;

        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;

        let payload: DiscordInteraction = serde_json::from_slice(raw_body)
            .map_err(|e| ChannelError::Transport(format!("discord parse: {e}")))?;
        let (sender, body) = payload.extract();

        Ok(ChannelMessage::new(&self.id, sender, body).with_taint(self.default_taint))
    }

    /// Drain the outbox.
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }
}

#[async_trait]
impl ChannelTrait for DiscordChannel {
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

/// The slice of the Discord interactions payload we model. The user
/// `username` lives at `member.user.username` for guild commands and
/// `user.username` for DM commands; we accept either path.
#[derive(Debug, Deserialize)]
struct DiscordInteraction {
    #[serde(default)]
    data: Option<DiscordInteractionData>,
    #[serde(default)]
    member: Option<DiscordMember>,
    #[serde(default)]
    user: Option<DiscordUser>,
}

#[derive(Debug, Deserialize)]
struct DiscordInteractionData {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    options: Option<Vec<DiscordOption>>,
}

#[derive(Debug, Deserialize)]
struct DiscordOption {
    #[serde(default)]
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct DiscordMember {
    user: Option<DiscordUser>,
}

#[derive(Debug, Deserialize)]
struct DiscordUser {
    username: Option<String>,
}

impl DiscordInteraction {
    fn extract(self) -> (String, String) {
        let sender = self
            .member
            .and_then(|m| m.user)
            .and_then(|u| u.username)
            .or_else(|| self.user.and_then(|u| u.username))
            .unwrap_or_else(|| "anonymous".into());
        let body = self
            .data
            .map(|d| {
                let name = d.name.unwrap_or_default();
                let value = d
                    .options
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|o| match o.value {
                        serde_json::Value::String(s) => Some(s),
                        v => Some(v.to_string()),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if value.is_empty() {
                    name
                } else {
                    format!("/{name} {value}")
                }
            })
            .unwrap_or_default();
        (sender, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemorySecretStore;
    use ed25519_dalek::{Signer, SigningKey};
    use gauss_kernel::PrivilegedKernel;
    use gaussclaw_agent::KernelHandle;
    use rand_core::OsRng;
    use std::sync::Arc;

    fn kernel() -> KernelHandle {
        KernelHandle::new(Arc::new(PrivilegedKernel::new(CapToken::TOP)))
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[tokio::test]
    async fn accepts_correctly_signed_interaction() {
        // Generate a real Ed25519 keypair and use the public key as the
        // discord bot key.
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let pk_hex = hex(pk.as_bytes());

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("DISCORD_PUBLIC_KEY", pk_hex.into_bytes());
        let ch = DiscordChannel::new(secrets, kernel(), "DISCORD_PUBLIC_KEY");

        let ts = "1700000000";
        let body = br#"{"member":{"user":{"username":"alice"}},"data":{"name":"ping"}}"#;
        let mut msg = Vec::new();
        msg.extend_from_slice(ts.as_bytes());
        msg.extend_from_slice(body);
        let sig = sk.sign(&msg);
        let sig_hex = hex(&sig.to_bytes());

        let m = ch.handle_webhook(ts, &sig_hex, body).await.expect("ok");
        assert_eq!(m.sender, "alice");
        assert_eq!(m.body, "ping");
        assert_eq!(m.taint, TaintLabel::Web);
    }

    #[tokio::test]
    async fn rejects_bad_signature() {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("DISCORD_PUBLIC_KEY", hex(pk.as_bytes()).into_bytes());
        let ch = DiscordChannel::new(secrets, kernel(), "DISCORD_PUBLIC_KEY");

        let bad_sig: String = std::iter::repeat('0').take(128).collect();
        let result = ch.handle_webhook("1700000000", &bad_sig, b"{}").await;
        assert!(matches!(result, Err(ChannelError::BadSignature)));
    }
}
