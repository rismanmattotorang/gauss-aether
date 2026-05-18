//! [`EmailChannel`] — typed email adapter.
//!
//! Inbound: an external mail receiver (postfix LDA hook, IMAP poll
//! daemon, SES-SNS bridge) parses raw RFC 5322 messages and feeds
//! [`EmailChannel::ingest_message`]. The adapter applies a configurable
//! sender allowlist before constructing the typed [`ChannelMessage`].
//!
//! Outbound: SMTP delivery is delegated to the surface plane (the
//! adapter queues structured outbound messages with the recipient and
//! body). The structural superiority over Hermes is the typed payload —
//! Hermes adapters pass dicts around with no schema; here every email
//! is a [`ParsedEmail`] that carries headers, body, and a fingerprint.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::KernelHandle;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    ChannelError, ChannelMessage, ChannelResult, ChannelTrait, OutboundMessage, SecretStore,
};

/// Email adapter.
pub struct EmailChannel {
    id: String,
    /// SMTP secret handle (resolved at outbound-flush time).
    smtp_secret_handle: String,
    secrets: Arc<dyn SecretStore>,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    /// Optional sender-address allowlist. Empty means "no filter".
    allowlist: BTreeSet<String>,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl EmailChannel {
    /// Build an email adapter. `smtp_secret_handle` typically resolves
    /// to `username:password` for the outbound SMTP relay.
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        kernel: KernelHandle,
        smtp_secret_handle: impl Into<String>,
    ) -> Self {
        Self {
            id: "email".into(),
            smtp_secret_handle: smtp_secret_handle.into(),
            secrets,
            kernel,
            // Email taint defaults to Adversarial — operators must opt
            // in to Web (e.g. by validating SPF / DKIM / DMARC out of
            // band) before downgrading.
            default_taint: TaintLabel::Adversarial,
            allowlist: BTreeSet::new(),
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Configure a sender-address allowlist. Senders not in the list
    /// produce [`ChannelError::BadSignature`] when ingested.
    #[must_use]
    pub fn with_sender_allowlist<I, S>(mut self, addrs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowlist = addrs
            .into_iter()
            .map(|s| s.into().to_ascii_lowercase())
            .collect();
        self
    }

    /// Downgrade the inbound taint floor (e.g. operator has verified
    /// SPF / DKIM / DMARC out of band and trusts the channel).
    #[must_use]
    pub const fn with_default_taint(mut self, taint: TaintLabel) -> Self {
        self.default_taint = taint;
        self
    }

    /// Ingest a parsed email and turn it into a typed channel message.
    ///
    /// # Errors
    /// [`ChannelError::BadSignature`] when the sender is not on the
    /// configured allowlist, [`ChannelError::Denied`] on kernel-admit
    /// refusal.
    pub async fn ingest_message(&self, mail: &ParsedEmail) -> ChannelResult<ChannelMessage> {
        if !self.allowlist.is_empty() && !self.allowlist.contains(&mail.from.to_ascii_lowercase()) {
            return Err(ChannelError::BadSignature);
        }
        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;
        Ok(
            ChannelMessage::new(&self.id, mail.from.clone(), mail.body.clone())
                .with_taint(self.default_taint)
                .with_meta("subject", serde_json::Value::String(mail.subject.clone()))
                .with_meta(
                    "message_id",
                    serde_json::Value::String(mail.message_id.clone()),
                ),
        )
    }

    /// Outbox accessor for the SMTP transport driver.
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }

    /// Resolve the SMTP secret at send time. Returns `None` if the
    /// secret store doesn't contain the configured handle.
    #[must_use]
    pub fn smtp_secret(&self) -> Option<Vec<u8>> {
        self.secrets.get(&self.smtp_secret_handle)
    }
}

#[async_trait]
impl ChannelTrait for EmailChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn required_caps(&self) -> CapToken {
        // Email ingest is a passive receiver — the SMTP daemon already
        // accepted the message. NETWORK_GET is the lowest admit gate
        // that still triggers a meaningful taint check; the kernel
        // refuses Adversarial → NETWORK_GET, but admits verified
        // (Web / User / Trusted) → NETWORK_GET.
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

/// Typed parsed email. Constructed by an external receiver (Postfix
/// hook, IMAP daemon, SES SNS bridge); the adapter never speaks SMTP /
/// IMAP itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParsedEmail {
    /// Envelope `From` address (lower-cased recommended).
    pub from: String,
    /// Subject header.
    pub subject: String,
    /// Plain-text body. HTML bodies should be flattened by the receiver.
    pub body: String,
    /// `Message-ID` header (used for deduplication on the agent side).
    pub message_id: String,
}

impl ParsedEmail {
    /// Convenience constructor.
    #[must_use]
    pub fn new(
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            from: from.into(),
            subject: subject.into(),
            body: body.into(),
            message_id: String::new(),
        }
    }

    /// Attach a `Message-ID` header.
    #[must_use]
    pub fn with_message_id(mut self, id: impl Into<String>) -> Self {
        self.message_id = id.into();
        self
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
    async fn default_adversarial_taint_is_refused_by_kernel() {
        // The contract: an unverified email lands at Adversarial taint;
        // the kernel admit gate refuses Adversarial → NETWORK_POST.
        // Operators downgrade only after verifying SPF / DKIM / DMARC.
        let ch = EmailChannel::new(Arc::new(InMemorySecretStore::default()), kernel(), "S");
        let mail = ParsedEmail::new("alice@example.com", "hi", "body");
        let result = ch.ingest_message(&mail).await;
        assert!(matches!(result, Err(ChannelError::Denied(_))));
    }

    #[tokio::test]
    async fn allowlist_admits_matching_sender_when_taint_downgraded() {
        let ch = EmailChannel::new(Arc::new(InMemorySecretStore::default()), kernel(), "S")
            .with_default_taint(TaintLabel::Web)
            .with_sender_allowlist(["alice@example.com"]);
        let mail = ParsedEmail::new("Alice@Example.com", "hi", "body");
        let m = ch.ingest_message(&mail).await.expect("ok");
        assert_eq!(m.sender, "Alice@Example.com"); // case preserved in message
    }

    #[tokio::test]
    async fn allowlist_rejects_unknown_sender() {
        let ch = EmailChannel::new(Arc::new(InMemorySecretStore::default()), kernel(), "S")
            .with_default_taint(TaintLabel::Web)
            .with_sender_allowlist(["alice@example.com"]);
        let mail = ParsedEmail::new("eve@evil.example", "hi", "body");
        let result = ch.ingest_message(&mail).await;
        assert!(matches!(result, Err(ChannelError::BadSignature)));
    }

    #[tokio::test]
    async fn downgraded_taint_is_applied() {
        let ch = EmailChannel::new(Arc::new(InMemorySecretStore::default()), kernel(), "S")
            .with_default_taint(TaintLabel::Web);
        let mail = ParsedEmail::new("alice@example.com", "hi", "body");
        let m = ch.ingest_message(&mail).await.expect("ok");
        assert_eq!(m.taint, TaintLabel::Web);
        assert_eq!(m.metadata["subject"], "hi");
    }

    #[tokio::test]
    async fn smtp_secret_resolves_at_send_time() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("SMTP_CREDS", b"user:pass".to_vec());
        let ch = EmailChannel::new(secrets, kernel(), "SMTP_CREDS");
        assert_eq!(ch.smtp_secret(), Some(b"user:pass".to_vec()));
    }
}
