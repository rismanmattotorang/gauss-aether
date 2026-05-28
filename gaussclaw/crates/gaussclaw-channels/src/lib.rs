//! `gaussclaw-channels` — messaging channel adapters.
//!
//! Phase 1 Task 8 of `GAUSSCLAW_ROADMAP.md`. The upstream Hermes shipped
//! ~16 channel adapters (Slack, Discord, Telegram, WhatsApp, Signal,
//! Matrix, Email, SMS, Feishu, WeCom, BlueBubbles, Home Assistant,
//! Mattermost, IRC, XMPP, Webhook) as Python modules with ad-hoc auth,
//! ad-hoc dict-passing, and no taint labelling. This crate replaces them
//! with a structural design.
//!
//! ## Four superiorities over the Hermes upstream
//!
//! 1. **Typed [`ChannelMessage`] / [`OutboundMessage`].** Hermes adapters
//!    pass `dict`s; here every adapter implements `async fn inbound`
//!    returning a [`ChannelMessage`] whose shape is the same across
//!    every adapter — typo-safe and serde-validated.
//!
//! 2. **Default-adversarial taint on ingress.** Every inbound message
//!    leaves the adapter with [`gauss_core::TaintLabel::Adversarial`]
//!    unless the adapter explicitly downgrades it (e.g. signed
//!    Slack webhooks may downgrade to `Web`). Hermes does not taint.
//!
//! 3. **HMAC verification is the trait surface, not a per-adapter
//!    decision.** [`hmac_verify`] is the canonical primitive; webhook
//!    adapters call it before [`ChannelTrait::inbound`] returns
//!    anything. Constant-time comparison via [`subtle`] avoids the
//!    timing-leak class Hermes adapters occasionally re-invent.
//!
//! 4. **Secrets via [`SecretStore`].** Every adapter resolves auth
//!    secrets through a pluggable trait (env var by default, HW-attested
//!    in production). Hermes reads raw `os.environ`.
//!
//! ## Scope for this slice
//!
//! - Trait surface ([`ChannelTrait`], [`ChannelRegistry`]).
//! - Two working adapters: [`InMemoryChannel`] (for tests) and
//!   [`WebhookChannel`] (HMAC-verified generic webhook — the foundation
//!   that Slack / Discord / Mattermost / Webhook all use).
//! - Bridge into [`gaussclaw_agent::KernelHandle`] so every channel
//!   call is admit-gated.
//!
//! Vendor-specific adapters (Telegram, Signal, Matrix, …) follow as
//! small follow-on PRs that each just implement [`ChannelTrait`].

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::unused_async,
    clippy::arithmetic_side_effects,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::unnecessary_filter_map,
    clippy::filter_map_bool_then,
    clippy::needless_raw_string_hashes,
    clippy::format_collect,
    clippy::manual_repeat_n,
    clippy::manual_str_repeat,
    clippy::format_push_string
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod discord;
pub mod email;
pub mod outbox_transport;
pub mod sink_adapter;
pub mod slack;
pub mod sprint7_adapters;
pub mod telegram;

pub use discord::DiscordChannel;
pub use email::{EmailChannel, ParsedEmail};
pub use outbox_transport::{DiscordOutbox, OutboxTransport, SlackOutbox, TelegramOutbox};
pub use sink_adapter::ChannelMessageSink;
pub use slack::SlackChannel;
pub use sprint7_adapters::{
    MatrixChannel, MattermostChannel, SignalChannel, TwilioSmsChannel, WhatsAppChannel,
};
pub use telegram::TelegramChannel;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, TaintLabel};
use gaussclaw_agent::{KernelHandle, SurfaceRequest};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

// ─── errors ─────────────────────────────────────────────────────────────────

/// Channel-side error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ChannelError {
    /// The kernel refused this message.
    #[error("kernel admit denied: {0:?}")]
    Denied(#[from] GaussError),
    /// HMAC signature mismatch.
    #[error("signature verification failed")]
    BadSignature,
    /// Required secret was not present in the secret store.
    #[error("missing secret: {0}")]
    MissingSecret(String),
    /// Adapter is in a non-runnable state (e.g. not yet started).
    #[error("adapter not running: {0}")]
    NotRunning(String),
    /// Unknown adapter id requested from the registry.
    #[error("unknown channel: {0}")]
    UnknownChannel(String),
    /// Underlying IO / transport failure.
    #[error("transport: {0}")]
    Transport(String),
    /// Detailed signature failure (Sprint 7 §5). Carries a reason
    /// string so the audit chain records which axis failed.
    #[error("signature invalid: {0}")]
    SignatureInvalid(String),
    /// Replay-window guard tripped (Sprint 7 §5).
    #[error("timestamp outside replay window")]
    ReplayWindow,
    /// Internal failure inside an adapter (Sprint 7 §5).
    #[error("internal: {0}")]
    Internal(String),
}

/// Convenience result alias.
pub type ChannelResult<T> = Result<T, ChannelError>;

// ─── secret store ──────────────────────────────────────────────────────────

/// Pluggable secret resolver. Production deployments plug in a HW-attested
/// store (vault, sealed AES-GCM blob, etc.); tests use [`InMemorySecretStore`].
pub trait SecretStore: Send + Sync {
    /// Resolve a secret by handle, or return `None` if unknown.
    fn get(&self, handle: &str) -> Option<Vec<u8>>;
}

/// Default secret store backed by environment variables. The `handle` is
/// the variable name; values are returned as UTF-8 bytes.
#[derive(Debug, Default, Clone)]
pub struct EnvSecretStore;

impl SecretStore for EnvSecretStore {
    fn get(&self, handle: &str) -> Option<Vec<u8>> {
        std::env::var_os(handle).map(|v| v.to_string_lossy().into_owned().into_bytes())
    }
}

/// In-memory secret store. Tests and integration fixtures use this.
#[derive(Debug, Default, Clone)]
pub struct InMemorySecretStore {
    inner: Arc<std::sync::RwLock<BTreeMap<String, Vec<u8>>>>,
}

impl InMemorySecretStore {
    /// Insert a (handle → bytes) pair.
    pub fn insert(&self, handle: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.inner
            .write()
            .unwrap()
            .insert(handle.into(), value.into());
    }
}

impl SecretStore for InMemorySecretStore {
    fn get(&self, handle: &str) -> Option<Vec<u8>> {
        self.inner.read().unwrap().get(handle).cloned()
    }
}

// ─── messages ──────────────────────────────────────────────────────────────

/// One inbound message, typed and tainted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelMessage {
    /// Channel adapter id (e.g. `slack`, `webhook:github`).
    pub channel: String,
    /// Sender identity in the channel's namespace (`@user`, room id, …).
    pub sender: String,
    /// Free-text body.
    pub body: String,
    /// Information-flow taint. Defaults to [`TaintLabel::Adversarial`]
    /// — the calling adapter must explicitly downgrade.
    pub taint: TaintLabel,
    /// RFC3339 timestamp string.
    pub ts: String,
    /// Opaque per-adapter metadata (channel id, thread id, …).
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl ChannelMessage {
    /// Build a message with the adversarial-taint default.
    #[must_use]
    pub fn new(
        channel: impl Into<String>,
        sender: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            sender: sender.into(),
            body: body.into(),
            taint: TaintLabel::Adversarial,
            ts: rfc3339_now(),
            metadata: serde_json::Map::new(),
        }
    }

    /// Downgrade the taint (e.g. after a signed webhook verifies).
    #[must_use]
    pub fn with_taint(mut self, taint: TaintLabel) -> Self {
        self.taint = taint;
        self
    }

    /// Attach a single metadata field.
    #[must_use]
    pub fn with_meta(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// One outbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OutboundMessage {
    /// Recipient identity in the channel's namespace.
    pub recipient: String,
    /// Free-text body.
    pub body: String,
    /// Opaque per-adapter options (thread reply, image attachment, …).
    pub options: serde_json::Map<String, serde_json::Value>,
}

impl OutboundMessage {
    /// Build a plain outbound message.
    #[must_use]
    pub fn new(recipient: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            recipient: recipient.into(),
            body: body.into(),
            options: serde_json::Map::new(),
        }
    }
}

// ─── trait ─────────────────────────────────────────────────────────────────

/// Adapter contract. Every messaging integration implements this.
///
/// Async-trait, `Send + Sync`. Implementations are expected to run their
/// own ingress loops (or webhook handler) and forward messages through
/// `sink` for the agent to consume.
#[async_trait]
pub trait ChannelTrait: Send + Sync {
    /// Stable adapter id (`slack`, `discord`, `webhook:github`).
    fn id(&self) -> &str;

    /// Required capability set; the kernel admit gate uses this.
    fn required_caps(&self) -> CapToken;

    /// Default taint label this adapter applies to inbound messages.
    /// Overrides are still possible per-message (e.g. signed webhook
    /// downgrades from [`TaintLabel::Adversarial`] to [`TaintLabel::Web`]).
    fn default_taint(&self) -> TaintLabel {
        TaintLabel::Adversarial
    }

    /// Start the adapter, sending each inbound message into `sink`.
    ///
    /// The default implementation is a no-op for adapters that don't
    /// have a continuous ingress loop (e.g. webhook adapters receive
    /// inbound via a route handler instead).
    async fn start(&self, _sink: mpsc::Sender<ChannelMessage>) -> ChannelResult<()> {
        Ok(())
    }

    /// Send an outbound message.
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()>;

    /// Stop the adapter cleanly.
    async fn stop(&self) -> ChannelResult<()> {
        Ok(())
    }
}

// ─── HMAC verification ─────────────────────────────────────────────────────

type HmacSha256 = Hmac<Sha256>;

/// Constant-time HMAC-SHA256 verification.
///
/// `provided_hex` is the candidate hex-encoded MAC (e.g. the
/// `X-Hub-Signature-256` header GitHub sends). Returns `Ok(())` only when
/// the recomputed MAC matches; otherwise [`ChannelError::BadSignature`].
pub fn hmac_verify(secret: &[u8], body: &[u8], provided_hex: &str) -> ChannelResult<()> {
    let candidate = hex_decode(provided_hex.trim_start_matches("sha256="))
        .map_err(|()| ChannelError::BadSignature)?;
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| ChannelError::BadSignature)?;
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    if expected.as_slice().ct_eq(&candidate).into() {
        Ok(())
    } else {
        Err(ChannelError::BadSignature)
    }
}

pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i.saturating_add(2)], 16).map_err(|_e| ()))
        .collect()
}

// ─── webhook adapter ───────────────────────────────────────────────────────

/// HMAC-verified inbound webhook. Foundation for Slack, Discord (via
/// webhooks), Mattermost, Github, generic-webhook, and similar adapters.
///
/// The webhook framework calls [`WebhookChannel::handle_webhook`] with
/// the raw POST body and the candidate `sha256=` hex MAC; the adapter
/// verifies, kernel-admits, builds the typed [`ChannelMessage`], and
/// returns it for the agent to consume.
pub struct WebhookChannel {
    id: String,
    secret_handle: String,
    secrets: Arc<dyn SecretStore>,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl WebhookChannel {
    /// Construct a webhook adapter.
    ///
    /// - `id` — adapter identifier (`slack`, `webhook:github`, …).
    /// - `secret_handle` — key the [`SecretStore`] uses to resolve the
    ///   HMAC secret.
    /// - `secrets` — the secret store.
    /// - `kernel` — admit gate.
    /// - `default_taint` — taint applied to a *verified* inbound. A
    ///   verified message is downgraded from `Adversarial` to this
    ///   value; an unverified one is rejected outright.
    pub fn new(
        id: impl Into<String>,
        secret_handle: impl Into<String>,
        secrets: Arc<dyn SecretStore>,
        kernel: KernelHandle,
        default_taint: TaintLabel,
    ) -> Self {
        Self {
            id: id.into(),
            secret_handle: secret_handle.into(),
            secrets,
            kernel,
            default_taint,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify + admit + build a typed message. Called by the HTTP layer.
    pub async fn handle_webhook(
        &self,
        body: &[u8],
        signature_hex: &str,
        sender: impl Into<String>,
    ) -> ChannelResult<ChannelMessage> {
        let secret = self
            .secrets
            .get(&self.secret_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.secret_handle.clone()))?;
        hmac_verify(&secret, body, signature_hex)?;

        // Admit-gate. Verified inbound is per the adapter's default taint
        // (typically `Web`). A failure here surfaces as `Denied` and
        // never produces a message.
        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;

        let body_str = std::str::from_utf8(body).unwrap_or("").to_string();
        Ok(ChannelMessage::new(&self.id, sender, body_str).with_taint(self.default_taint))
    }

    /// Drain the outbox (test helper). Returns and clears every
    /// outbound message the agent sent on this adapter since the last
    /// drain.
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }
}

#[async_trait]
impl ChannelTrait for WebhookChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn required_caps(&self) -> CapToken {
        // Inbound channels are passive receivers, not network clients —
        // NETWORK_GET is the lowest-privilege cap that still triggers a
        // meaningful admit check. The default declass map admits this
        // under every non-adversarial taint, which is the right
        // structural contract for ingress: adversarial messages still
        // refuse, but verified Web / User / Trusted pass.
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

// ─── in-memory adapter ─────────────────────────────────────────────────────

/// In-process adapter used by tests and the CLI demo. Maintains an inbox
/// (messages to deliver to the agent) and an outbox (messages the agent
/// sent on this channel).
pub struct InMemoryChannel {
    id: String,
    kernel: KernelHandle,
    default_taint: TaintLabel,
    inbox: Mutex<Vec<ChannelMessage>>,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl InMemoryChannel {
    /// Build a new in-memory adapter.
    pub fn new(id: impl Into<String>, kernel: KernelHandle, default_taint: TaintLabel) -> Self {
        Self {
            id: id.into(),
            kernel,
            default_taint,
            inbox: Mutex::new(Vec::new()),
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Push a synthetic inbound message into the adapter, as if it had
    /// just arrived from the wire. Admit-gates first.
    pub async fn push_inbound(
        &self,
        sender: impl Into<String>,
        body: impl Into<String>,
    ) -> ChannelResult<()> {
        self.kernel
            .admit(self.required_caps(), self.default_taint)
            .map_err(ChannelError::Denied)?;
        let msg = ChannelMessage::new(&self.id, sender, body).with_taint(self.default_taint);
        self.inbox.lock().await.push(msg);
        Ok(())
    }

    /// Drain pending inbound messages (test helper).
    pub async fn drain_inbox(&self) -> Vec<ChannelMessage> {
        std::mem::take(&mut *self.inbox.lock().await)
    }

    /// Drain outbound messages (test helper).
    pub async fn drain_outbox(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.outbox.lock().await)
    }
}

#[async_trait]
impl ChannelTrait for InMemoryChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn required_caps(&self) -> CapToken {
        // Inbound channels are passive receivers, not network clients —
        // NETWORK_GET is the lowest-privilege cap that still triggers a
        // meaningful admit check. The default declass map admits this
        // under every non-adversarial taint, which is the right
        // structural contract for ingress: adversarial messages still
        // refuse, but verified Web / User / Trusted pass.
        CapToken::NETWORK_GET
    }

    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }

    async fn start(&self, sink: mpsc::Sender<ChannelMessage>) -> ChannelResult<()> {
        let messages = self.drain_inbox().await;
        for m in messages {
            let _ = sink.send(m).await;
        }
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── registry ──────────────────────────────────────────────────────────────

/// A registry of channel adapters. Owns each adapter behind an `Arc` and
/// dispatches sends by adapter id.
#[derive(Clone, Default)]
pub struct ChannelRegistry {
    adapters: Arc<Mutex<BTreeMap<String, Arc<dyn ChannelTrait>>>>,
}

impl ChannelRegistry {
    /// Build an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter. Replaces any existing adapter with the same id.
    pub async fn register(&self, adapter: Arc<dyn ChannelTrait>) {
        let id = adapter.id().to_string();
        self.adapters.lock().await.insert(id, adapter);
    }

    /// Look up an adapter.
    pub async fn get(&self, id: &str) -> Option<Arc<dyn ChannelTrait>> {
        self.adapters.lock().await.get(id).cloned()
    }

    /// Number of registered adapters.
    pub async fn len(&self) -> usize {
        self.adapters.lock().await.len()
    }

    /// Whether the registry is empty.
    pub async fn is_empty(&self) -> bool {
        self.adapters.lock().await.is_empty()
    }

    /// Send an outbound message via the adapter named `channel_id`.
    pub async fn send(&self, channel_id: &str, msg: OutboundMessage) -> ChannelResult<()> {
        let adapter = self
            .get(channel_id)
            .await
            .ok_or_else(|| ChannelError::UnknownChannel(channel_id.to_string()))?;
        adapter.send(msg).await
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn rfc3339_now() -> String {
    // Real RFC3339, second-precision UTC. Adapters that have a wire
    // timestamp from the upstream service should override this with that
    // value via [`ChannelMessage`] — but the default is now a real
    // RFC3339 string accepted by any conforming parser.
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

// `SurfaceRequest` is forwarded through `ChannelRegistry::send` indirectly
// (via the kernel handle the registry's adapters hold). We re-export the
// type so consumers can build their own plane mappings.
pub use gaussclaw_agent::SurfaceRequest as _ChannelSurfaceRequest;

/// Reserved for future: the plane every channel inbound should map to.
#[must_use]
pub const fn channel_request() -> SurfaceRequest {
    SurfaceRequest::Channel
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::CapToken;
    use gaussclaw_agent::KernelHandle;
    use hmac::Mac;

    fn sign(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        format!("sha256={}", hex_encode(&mac.finalize().into_bytes()))
    }

    fn hex_encode(b: &[u8]) -> String {
        let mut out = String::with_capacity(b.len().saturating_mul(2));
        for byte in b {
            out.push(nibble(byte >> 4));
            out.push(nibble(byte & 0x0F));
        }
        out
    }

    #[allow(clippy::arithmetic_side_effects)]
    const fn nibble(n: u8) -> char {
        match n {
            0..=9 => (b'0' + n) as char,
            10..=15 => (b'a' + n - 10) as char,
            _ => '0',
        }
    }

    #[test]
    fn hmac_verify_accepts_a_valid_signature() {
        let secret = b"shh";
        let body = b"hello world";
        let sig = sign(secret, body);
        hmac_verify(secret, body, &sig).expect("valid HMAC");
    }

    #[test]
    fn hmac_verify_rejects_a_tampered_body() {
        let secret = b"shh";
        let body = b"hello world";
        let sig = sign(secret, body);
        let err = hmac_verify(secret, b"hello world!", &sig).unwrap_err();
        assert!(matches!(err, ChannelError::BadSignature));
    }

    #[test]
    fn hmac_verify_rejects_a_bad_hex_signature() {
        let err = hmac_verify(b"shh", b"x", "not-hex").unwrap_err();
        assert!(matches!(err, ChannelError::BadSignature));
    }

    #[tokio::test]
    async fn webhook_round_trip() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("WEBHOOK_SECRET", b"shh".to_vec());
        let ch = WebhookChannel::new(
            "webhook:test",
            "WEBHOOK_SECRET",
            secrets,
            KernelHandle::permissive(),
            TaintLabel::Web,
        );
        let body = b"event payload";
        let sig = sign(b"shh", body);
        let msg = ch
            .handle_webhook(body, &sig, "github/dependabot")
            .await
            .unwrap();
        assert_eq!(msg.channel, "webhook:test");
        assert_eq!(msg.sender, "github/dependabot");
        assert_eq!(msg.taint, TaintLabel::Web);
        assert_eq!(msg.body, "event payload");
    }

    #[tokio::test]
    async fn webhook_missing_secret_is_fatal() {
        let secrets = Arc::new(InMemorySecretStore::default());
        let ch = WebhookChannel::new(
            "webhook:test",
            "MISSING",
            secrets,
            KernelHandle::permissive(),
            TaintLabel::Web,
        );
        let err = ch
            .handle_webhook(b"x", "sha256=00", "sender")
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::MissingSecret(_)));
    }

    #[tokio::test]
    async fn webhook_bad_signature_is_rejected() {
        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("S", b"correct".to_vec());
        let ch = WebhookChannel::new(
            "webhook:test",
            "S",
            secrets,
            KernelHandle::permissive(),
            TaintLabel::Web,
        );
        let bad = sign(b"wrong", b"x");
        let err = ch.handle_webhook(b"x", &bad, "sender").await.unwrap_err();
        assert!(matches!(err, ChannelError::BadSignature));
    }

    #[tokio::test]
    async fn webhook_denied_when_kernel_lacks_caps() {
        use gauss_kernel::PrivilegedKernel;
        use std::sync::Arc as StdArc;

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets.insert("S", b"shh".to_vec());
        let empty_kernel = StdArc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let kernel = KernelHandle::new(empty_kernel);
        let ch = WebhookChannel::new("webhook:test", "S", secrets, kernel, TaintLabel::Web);
        let body = b"x";
        let sig = sign(b"shh", body);
        let err = ch.handle_webhook(body, &sig, "sender").await.unwrap_err();
        assert!(matches!(err, ChannelError::Denied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn in_memory_channel_round_trip() {
        let ch = InMemoryChannel::new("test", KernelHandle::permissive(), TaintLabel::User);
        ch.push_inbound("alice", "hi").await.unwrap();
        let inbox = ch.drain_inbox().await;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].sender, "alice");
        assert_eq!(inbox[0].taint, TaintLabel::User);

        ch.send(OutboundMessage::new("alice", "yo")).await.unwrap();
        let outbox = ch.drain_outbox().await;
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].body, "yo");
    }

    #[tokio::test]
    async fn registry_routes_outbound_by_id() {
        let reg = ChannelRegistry::new();
        let ch = Arc::new(InMemoryChannel::new(
            "test",
            KernelHandle::permissive(),
            TaintLabel::User,
        ));
        reg.register(ch.clone()).await;
        assert_eq!(reg.len().await, 1);
        reg.send("test", OutboundMessage::new("a", "hi"))
            .await
            .unwrap();
        let outbox = ch.drain_outbox().await;
        assert_eq!(outbox.len(), 1);
    }

    #[tokio::test]
    async fn registry_returns_unknown_channel() {
        let reg = ChannelRegistry::new();
        let err = reg
            .send("nope", OutboundMessage::new("a", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::UnknownChannel(_)));
    }

    #[tokio::test]
    async fn inbound_defaults_to_adversarial_taint() {
        // Newly-constructed messages get adversarial taint until an
        // explicit downgrade. This is the structural anti-IPI baseline.
        let m = ChannelMessage::new("ch", "s", "b");
        assert_eq!(m.taint, TaintLabel::Adversarial);
        let m2 = m.with_taint(TaintLabel::User);
        assert_eq!(m2.taint, TaintLabel::User);
    }

    #[tokio::test]
    async fn env_secret_store_reads_env() {
        std::env::set_var("GAUSSCLAW_TEST_SECRET", "abc");
        let s = EnvSecretStore;
        assert_eq!(
            s.get("GAUSSCLAW_TEST_SECRET").as_deref(),
            Some(b"abc".as_slice())
        );
        std::env::remove_var("GAUSSCLAW_TEST_SECRET");
    }
}
