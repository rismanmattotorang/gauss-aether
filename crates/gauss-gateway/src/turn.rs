//! `POST /v1/turn` + `GET /v1/health` request / response shapes.

use gauss_core::{Action, TaintLabel, TurnId};
use serde::{Deserialize, Serialize};

/// `POST /v1/turn` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TurnRequest {
    /// Operator-supplied turn id; the gateway returns the same id in
    /// the response so clients can correlate.
    pub turn_id: TurnId,
    /// Free-text observation body.
    pub body: String,
    /// Information-flow taint hint. Defaults to [`TaintLabel::User`]
    /// when the field is omitted on the wire.
    #[serde(default = "default_user_taint")]
    pub taint: TaintLabel,
    /// Optional channel identifier (e.g. `"telegram"`, `"slack"`).
    #[serde(default)]
    pub channel: Option<String>,
}

const fn default_user_taint() -> TaintLabel {
    TaintLabel::User
}

impl TurnRequest {
    /// Construct.
    #[must_use]
    pub fn new(turn_id: TurnId, body: impl Into<String>) -> Self {
        Self {
            turn_id,
            body: body.into(),
            taint: TaintLabel::User,
            channel: None,
        }
    }

    /// Tag the channel.
    #[must_use]
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Override the taint.
    #[must_use]
    pub const fn with_taint(mut self, taint: TaintLabel) -> Self {
        self.taint = taint;
        self
    }
}

/// `POST /v1/turn` response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TurnResponse {
    /// Echo of the originating turn id.
    pub turn_id: TurnId,
    /// Actions the policy emitted.
    pub actions: Vec<Action>,
    /// Chain head digest (hex) after the WAL append.
    pub chain_head_hex: String,
    /// Chain length after the WAL append.
    pub chain_length: u64,
    /// Optional signed receipt (Phase 5) as opaque base64 — clients that
    /// want to verify decode this through the public verifier API.
    #[serde(default)]
    pub signed_receipt_b64: Option<String>,
}

impl TurnResponse {
    /// Construct an OK response.
    #[must_use]
    pub fn ok(
        turn_id: TurnId,
        actions: Vec<Action>,
        chain_head_hex: impl Into<String>,
        chain_length: u64,
    ) -> Self {
        Self {
            turn_id,
            actions,
            chain_head_hex: chain_head_hex.into(),
            chain_length,
            signed_receipt_b64: None,
        }
    }

    /// Attach the signed-receipt blob.
    #[must_use]
    pub fn with_signed_receipt(mut self, b64: impl Into<String>) -> Self {
        self.signed_receipt_b64 = Some(b64.into());
        self
    }
}

/// `GET /v1/health` response body — the `gauss-health` report serialised
/// as opaque JSON so the gateway crate doesn't take a hard dep on
/// `gauss-health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HealthResponse {
    /// Verbatim JSON body of the health report.
    pub report: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{TextAction, TurnId};

    #[test]
    fn turn_request_round_trips_with_default_taint() {
        let r = TurnRequest::new(TurnId::new(1), "hi");
        let s = serde_json::to_string(&r).unwrap();
        let back: TurnRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.turn_id, TurnId::new(1));
        assert_eq!(back.body, "hi");
    }

    #[test]
    fn missing_taint_defaults_to_user() {
        let json = r#"{"turn_id":7,"body":"hi"}"#;
        let r: TurnRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.taint, TaintLabel::User);
    }

    #[test]
    fn turn_response_round_trips() {
        let r = TurnResponse::ok(
            TurnId::new(2),
            vec![Action::Text(TextAction::new("hello"))],
            "abcdef",
            42,
        )
        .with_signed_receipt("base64-payload");
        let s = serde_json::to_string(&r).unwrap();
        let back: TurnResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.chain_length, 42);
        assert_eq!(back.signed_receipt_b64.as_deref(), Some("base64-payload"));
    }
}
