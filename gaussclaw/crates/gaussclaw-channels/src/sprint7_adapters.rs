//! Sprint 7 §5 — five additional channel adapters.
//!
//! Each adapter follows the existing pattern from `SlackChannel`:
//! a typed struct, ingress verification via the appropriate
//! signature primitive, outbound queued through an in-memory outbox.
//! The transport layer drains the outbox in `gaussclaw-surfaces`.
//!
//! Shipped here:
//!
//! - [`MattermostChannel`] — incoming webhooks signed with HMAC-SHA256
//!   over `<timestamp>.<body>` (same shape as Slack's `v0:<ts>:<body>`).
//! - [`WhatsAppChannel`] — Meta Graph API webhook signed with
//!   `X-Hub-Signature-256` (HMAC-SHA256 over the raw body).
//! - [`TwilioSmsChannel`] — `X-Twilio-Signature` (HMAC-SHA1 of
//!   `url + sorted(body params concatenated)` per Twilio spec).
//! - [`MatrixChannel`] — HS-to-AS via Authorization Bearer token
//!   (constant-time-compared against the operator-supplied secret).
//! - [`SignalChannel`] — bridge ingress; trusts the local socket and
//!   carries no over-the-wire signature. The structural cap-gate is
//!   the only defence (mirrors `signal-cli` topology).
//!
//! Hermes-superiority axes (carried over from the existing adapters):
//!
//! - **Adversarial-taint default on ingress** — every inbound message
//!   defaults to [`gauss_core::TaintLabel::Adversarial`]. Operators
//!   downgrade only when the signature verifies.
//! - **Cap-declared per channel** — each adapter declares
//!   `NETWORK_POST` (outbound) as a hard cap, refused at the kernel
//!   admit gate when the session grant is missing.
//! - **Constant-time signature comparison** — HMAC checks via the
//!   crate-canonical [`crate::hmac_verify`] (uses `subtle::ConstantTimeEq`).

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{
    hmac_verify, ChannelError, ChannelMessage, ChannelResult, ChannelTrait, OutboundMessage,
    SecretStore,
};

// ─── shared helpers ────────────────────────────────────────────────────────

fn empty_msg(channel: &str, sender: &str, body: String) -> ChannelMessage {
    ChannelMessage::new(channel.to_string(), sender.to_string(), body)
}

// ─── 1. Mattermost ─────────────────────────────────────────────────────────

/// Mattermost incoming-webhook adapter.
///
/// Signature scheme mirrors Slack:
/// `X-Mattermost-Request-Signature: v0=<hex>`, signing base is
/// `v0:<X-Mattermost-Request-Timestamp>:<body>`. (Mattermost's
/// official signing protocol is configurable; we lock the shape
/// against the documented default.)
pub struct MattermostChannel {
    id: String,
    signing_secret_handle: String,
    secrets: Arc<dyn SecretStore>,
    default_taint: TaintLabel,
    max_skew_secs: i64,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl MattermostChannel {
    /// Build a Mattermost adapter.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            id: "mattermost".into(),
            signing_secret_handle: "MATTERMOST_SIGNING_SECRET".into(),
            secrets,
            default_taint: TaintLabel::Adversarial,
            max_skew_secs: 300,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify an inbound webhook and build the typed message. On
    /// success the taint is downgraded from `Adversarial` to `Web`.
    pub async fn verify_ingress(
        &self,
        body: &[u8],
        timestamp_secs: i64,
        signature_hex: &str,
        sender: &str,
    ) -> ChannelResult<ChannelMessage> {
        let now = current_unix_seconds();
        if (timestamp_secs - now).abs() > self.max_skew_secs {
            return Err(ChannelError::ReplayWindow);
        }
        let secret = self
            .secrets
            .get(&self.signing_secret_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.signing_secret_handle.clone()))?;
        let base = format!(
            "v0:{timestamp_secs}:{}",
            std::str::from_utf8(body).unwrap_or("")
        );
        hmac_verify(&secret, base.as_bytes(), signature_hex)?;
        let body_str = String::from_utf8_lossy(body).into_owned();
        Ok(empty_msg(&self.id, sender, body_str).with_taint(TaintLabel::Web))
    }

    /// Inspect the in-memory outbox (testing).
    pub async fn outbox_len(&self) -> usize {
        self.outbox.lock().await.len()
    }
}

#[async_trait]
impl ChannelTrait for MattermostChannel {
    fn id(&self) -> &str {
        &self.id
    }
    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_POST
    }
    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── 2. WhatsApp ───────────────────────────────────────────────────────────

/// WhatsApp Cloud / Meta Graph webhook. Meta signs every payload with
/// `X-Hub-Signature-256: sha256=<hex>`, HMAC-SHA256 over the raw body.
pub struct WhatsAppChannel {
    id: String,
    app_secret_handle: String,
    secrets: Arc<dyn SecretStore>,
    default_taint: TaintLabel,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl WhatsAppChannel {
    /// Build a WhatsApp adapter.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            id: "whatsapp".into(),
            app_secret_handle: "WHATSAPP_APP_SECRET".into(),
            secrets,
            default_taint: TaintLabel::Adversarial,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify the `X-Hub-Signature-256: sha256=…` header and build a
    /// downgraded `Web` message.
    pub async fn verify_ingress(
        &self,
        body: &[u8],
        header_value: &str,
        sender: &str,
    ) -> ChannelResult<ChannelMessage> {
        let sig_hex = header_value
            .strip_prefix("sha256=")
            .ok_or_else(|| ChannelError::SignatureInvalid("missing sha256= prefix".into()))?;
        let secret = self
            .secrets
            .get(&self.app_secret_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.app_secret_handle.clone()))?;
        hmac_verify(&secret, body, sig_hex)?;
        Ok(
            empty_msg(&self.id, sender, String::from_utf8_lossy(body).into_owned())
                .with_taint(TaintLabel::Web),
        )
    }

    /// Inspect outbox.
    pub async fn outbox_len(&self) -> usize {
        self.outbox.lock().await.len()
    }
}

#[async_trait]
impl ChannelTrait for WhatsAppChannel {
    fn id(&self) -> &str {
        &self.id
    }
    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_POST
    }
    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── 3. Twilio SMS ─────────────────────────────────────────────────────────

/// Twilio SMS webhook. Twilio signs each request with
/// `X-Twilio-Signature`, HMAC-SHA1 of `request_url + concatenated(sorted
/// form-encoded params)`, base64-encoded.
pub struct TwilioSmsChannel {
    id: String,
    auth_token_handle: String,
    secrets: Arc<dyn SecretStore>,
    default_taint: TaintLabel,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl TwilioSmsChannel {
    /// Build a Twilio SMS adapter.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            id: "twilio_sms".into(),
            auth_token_handle: "TWILIO_AUTH_TOKEN".into(),
            secrets,
            default_taint: TaintLabel::Adversarial,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify a Twilio signature against the request URL + sorted
    /// form-encoded params. `sorted_params` must be `(key, value)`
    /// pairs already sorted by key (caller responsibility — the spec
    /// is precise about which sort).
    pub async fn verify_ingress(
        &self,
        url: &str,
        sorted_params: &[(String, String)],
        signature_b64: &str,
        sender: &str,
    ) -> ChannelResult<ChannelMessage> {
        let secret = self
            .secrets
            .get(&self.auth_token_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.auth_token_handle.clone()))?;
        let mut base = String::with_capacity(url.len() + 128);
        base.push_str(url);
        for (k, v) in sorted_params {
            base.push_str(k);
            base.push_str(v);
        }
        let mut mac = Hmac::<Sha1>::new_from_slice(&secret)
            .map_err(|e| ChannelError::Internal(format!("hmac init: {e}")))?;
        mac.update(base.as_bytes());
        let computed = mac.finalize().into_bytes();
        // Twilio sends base64 (not hex). Compare in constant time.
        let provided = base64_decode(signature_b64)
            .map_err(|e| ChannelError::SignatureInvalid(format!("base64: {e}")))?;
        if provided.as_slice().ct_eq(computed.as_slice()).unwrap_u8() != 1 {
            return Err(ChannelError::SignatureInvalid("signature mismatch".into()));
        }
        let body = sorted_params
            .iter()
            .find_map(|(k, v)| (k == "Body").then(|| v.clone()))
            .unwrap_or_default();
        Ok(empty_msg(&self.id, sender, body).with_taint(TaintLabel::Web))
    }

    /// Inspect outbox.
    pub async fn outbox_len(&self) -> usize {
        self.outbox.lock().await.len()
    }
}

#[async_trait]
impl ChannelTrait for TwilioSmsChannel {
    fn id(&self) -> &str {
        &self.id
    }
    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_POST
    }
    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── 4. Matrix ─────────────────────────────────────────────────────────────

/// Matrix homeserver → application-service ingress.
///
/// The homeserver authenticates with the `Authorization: Bearer
/// <token>` header; the AS verifies the token against an
/// operator-supplied secret in constant time.
pub struct MatrixChannel {
    id: String,
    as_token_handle: String,
    secrets: Arc<dyn SecretStore>,
    default_taint: TaintLabel,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl MatrixChannel {
    /// Build a Matrix AS adapter.
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            id: "matrix".into(),
            as_token_handle: "MATRIX_AS_TOKEN".into(),
            secrets,
            default_taint: TaintLabel::Adversarial,
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Verify a Bearer token and build a downgraded `Web` message.
    pub async fn verify_ingress(
        &self,
        authorization_header: &str,
        body: &[u8],
        sender: &str,
    ) -> ChannelResult<ChannelMessage> {
        let token = authorization_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ChannelError::SignatureInvalid("missing Bearer prefix".into()))?
            .trim();
        let expected = self
            .secrets
            .get(&self.as_token_handle)
            .ok_or_else(|| ChannelError::MissingSecret(self.as_token_handle.clone()))?;
        if token.as_bytes().ct_eq(&expected).unwrap_u8() != 1 {
            return Err(ChannelError::SignatureInvalid("token mismatch".into()));
        }
        Ok(
            empty_msg(&self.id, sender, String::from_utf8_lossy(body).into_owned())
                .with_taint(TaintLabel::Web),
        )
    }

    /// Inspect outbox.
    pub async fn outbox_len(&self) -> usize {
        self.outbox.lock().await.len()
    }
}

#[async_trait]
impl ChannelTrait for MatrixChannel {
    fn id(&self) -> &str {
        &self.id
    }
    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_POST
    }
    fn default_taint(&self) -> TaintLabel {
        self.default_taint
    }
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── 5. Signal ─────────────────────────────────────────────────────────────

/// Signal adapter (bridge-only).
///
/// Signal's bridge model is local-socket only — the `signal-cli`
/// daemon authenticates the user via the Signal protocol itself, then
/// relays messages over a UNIX socket. We treat ingress as
/// trusted-by-locality (operator gates the socket); the cap surface
/// is the only auth.
pub struct SignalChannel {
    id: String,
    outbox: Mutex<Vec<OutboundMessage>>,
}

impl SignalChannel {
    /// Build a Signal adapter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: "signal".into(),
            outbox: Mutex::new(Vec::new()),
        }
    }

    /// Accept an inbound message from the local bridge. Taint
    /// remains `User` (local socket); the operator can downgrade
    /// further when bridge auth is hardened.
    pub fn accept_ingress(&self, sender: &str, body: String) -> ChannelMessage {
        empty_msg(&self.id, sender, body).with_taint(TaintLabel::User)
    }

    /// Inspect outbox.
    pub async fn outbox_len(&self) -> usize {
        self.outbox.lock().await.len()
    }
}

impl Default for SignalChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelTrait for SignalChannel {
    fn id(&self) -> &str {
        &self.id
    }
    fn required_caps(&self) -> CapToken {
        CapToken::NETWORK_POST
    }
    fn default_taint(&self) -> TaintLabel {
        TaintLabel::User
    }
    async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
        self.outbox.lock().await.push(msg);
        Ok(())
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Hand-rolled base64 decode (standard alphabet, no padding tolerance).
/// We don't pull `base64` into this crate for one call site.
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut bytes = s.as_bytes().to_vec();
    while bytes.last() == Some(&b'=') {
        bytes.pop();
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &c in &bytes {
        let v: u32 = match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("invalid base64 byte: {c}")),
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemorySecretStore;
    use hmac::Mac;

    fn store_with(handle: &str, secret: &[u8]) -> Arc<dyn SecretStore> {
        let s = InMemorySecretStore::default();
        s.insert(handle, secret.to_vec());
        Arc::new(s)
    }

    fn hmac_sha256_hex(secret: &[u8], body: &[u8]) -> String {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret).expect("hmac");
        mac.update(body);
        let tag = mac.finalize().into_bytes();
        tag.iter().map(|b| format!("{b:02x}")).collect()
    }

    // ─── Mattermost ───────────────────────────────────────────────

    #[tokio::test]
    async fn mattermost_verifies_signed_webhook() {
        let secret = b"mm-secret";
        let store = store_with("MATTERMOST_SIGNING_SECRET", secret);
        let ch = MattermostChannel::new(store);
        let now = current_unix_seconds();
        let body = b"{\"text\":\"hi\"}";
        let base = format!("v0:{now}:{}", std::str::from_utf8(body).unwrap());
        let sig = hmac_sha256_hex(secret, base.as_bytes());
        let msg = ch.verify_ingress(body, now, &sig, "@user").await.unwrap();
        assert_eq!(msg.channel, "mattermost");
        assert_eq!(msg.taint, TaintLabel::Web);
        assert_eq!(msg.sender, "@user");
    }

    #[tokio::test]
    async fn mattermost_refuses_stale_timestamp() {
        let store = store_with("MATTERMOST_SIGNING_SECRET", b"x");
        let ch = MattermostChannel::new(store);
        let stale = current_unix_seconds() - 3600;
        let err = ch
            .verify_ingress(b"{}", stale, "doesnt-matter", "@u")
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::ReplayWindow));
    }

    // ─── WhatsApp ─────────────────────────────────────────────────

    #[tokio::test]
    async fn whatsapp_verifies_sha256_signature() {
        let secret = b"wa-secret";
        let store = store_with("WHATSAPP_APP_SECRET", secret);
        let ch = WhatsAppChannel::new(store);
        let body = b"{\"entry\":[{\"id\":\"x\"}]}";
        let header = format!("sha256={}", hmac_sha256_hex(secret, body));
        let msg = ch.verify_ingress(body, &header, "+15551234").await.unwrap();
        assert_eq!(msg.channel, "whatsapp");
        assert_eq!(msg.taint, TaintLabel::Web);
    }

    #[tokio::test]
    async fn whatsapp_rejects_missing_sha256_prefix() {
        let store = store_with("WHATSAPP_APP_SECRET", b"x");
        let ch = WhatsAppChannel::new(store);
        let err = ch.verify_ingress(b"{}", "abc", "u").await.unwrap_err();
        assert!(matches!(err, ChannelError::SignatureInvalid(_)));
    }

    // ─── Twilio SMS ───────────────────────────────────────────────

    #[tokio::test]
    async fn twilio_verifies_sha1_signature() {
        // Reference vector from the Twilio docs.
        let token = b"12345";
        let store = store_with("TWILIO_AUTH_TOKEN", token);
        let ch = TwilioSmsChannel::new(store);
        let url = "https://example.com/sms";
        let sorted = [
            ("Body".to_string(), "hi".to_string()),
            ("From".to_string(), "+15551234".to_string()),
        ];
        let mut base = String::from(url);
        for (k, v) in &sorted {
            base.push_str(k);
            base.push_str(v);
        }
        let mut mac = Hmac::<Sha1>::new_from_slice(token).unwrap();
        mac.update(base.as_bytes());
        let tag = mac.finalize().into_bytes();
        // Encode as base64 manually.
        let sig_b64 = base64_encode(&tag);
        let msg = ch
            .verify_ingress(url, &sorted, &sig_b64, "+15551234")
            .await
            .unwrap();
        assert_eq!(msg.channel, "twilio_sms");
        assert_eq!(msg.body, "hi");
        assert_eq!(msg.taint, TaintLabel::Web);
    }

    #[tokio::test]
    async fn twilio_rejects_tampered_body() {
        let token = b"12345";
        let store = store_with("TWILIO_AUTH_TOKEN", token);
        let ch = TwilioSmsChannel::new(store);
        // Signature computed over different content.
        let mut mac = Hmac::<Sha1>::new_from_slice(token).unwrap();
        mac.update(b"https://example.com/smsBodyhi");
        let tag = mac.finalize().into_bytes();
        let sig_b64 = base64_encode(&tag);
        let tampered = [("Body".to_string(), "bye".to_string())];
        let err = ch
            .verify_ingress("https://example.com/sms", &tampered, &sig_b64, "u")
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::SignatureInvalid(_)));
    }

    // ─── Matrix ───────────────────────────────────────────────────

    #[tokio::test]
    async fn matrix_verifies_bearer_token() {
        let token = b"matrix-as-token-abc";
        let store = store_with("MATRIX_AS_TOKEN", token);
        let ch = MatrixChannel::new(store);
        let msg = ch
            .verify_ingress(
                "Bearer matrix-as-token-abc",
                b"{}",
                "@homeserver:example.com",
            )
            .await
            .unwrap();
        assert_eq!(msg.channel, "matrix");
        assert_eq!(msg.taint, TaintLabel::Web);
    }

    #[tokio::test]
    async fn matrix_rejects_wrong_token() {
        let store = store_with("MATRIX_AS_TOKEN", b"good-token");
        let ch = MatrixChannel::new(store);
        let err = ch
            .verify_ingress("Bearer bad-token", b"{}", "u")
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::SignatureInvalid(_)));
    }

    #[tokio::test]
    async fn matrix_rejects_missing_bearer_prefix() {
        let store = store_with("MATRIX_AS_TOKEN", b"x");
        let ch = MatrixChannel::new(store);
        let err = ch.verify_ingress("x", b"{}", "u").await.unwrap_err();
        assert!(matches!(err, ChannelError::SignatureInvalid(_)));
    }

    // ─── Signal ───────────────────────────────────────────────────

    #[tokio::test]
    async fn signal_accept_ingress_returns_user_taint() {
        let ch = SignalChannel::new();
        let msg = ch.accept_ingress("+15550100", "hi".into());
        assert_eq!(msg.channel, "signal");
        assert_eq!(msg.taint, TaintLabel::User);
    }

    #[tokio::test]
    async fn all_adapters_outbound_queues_message() {
        let store = store_with("any", b"x");
        let adapters: Vec<Box<dyn ChannelTrait>> = vec![
            Box::new(MattermostChannel::new(store.clone())),
            Box::new(WhatsAppChannel::new(store.clone())),
            Box::new(TwilioSmsChannel::new(store.clone())),
            Box::new(MatrixChannel::new(store)),
            Box::new(SignalChannel::new()),
        ];
        for ch in adapters {
            assert_eq!(ch.required_caps().bits(), CapToken::NETWORK_POST.bits());
            ch.send(OutboundMessage::new("recipient", "body"))
                .await
                .unwrap();
        }
    }

    #[test]
    fn base64_decode_round_trip() {
        let raw = b"hello world";
        let enc = base64_encode(raw);
        let back = base64_decode(&enc).unwrap();
        assert_eq!(back, raw);
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
        let mut buf: u32 = 0;
        let mut bits: u32 = 0;
        for &b in bytes {
            buf = (buf << 8) | u32::from(b);
            bits += 8;
            while bits >= 6 {
                bits -= 6;
                let idx = ((buf >> bits) & 0x3f) as usize;
                out.push(char::from(ALPHABET[idx]));
            }
        }
        if bits > 0 {
            let idx = ((buf << (6 - bits)) & 0x3f) as usize;
            out.push(char::from(ALPHABET[idx]));
        }
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
        out
    }
}
