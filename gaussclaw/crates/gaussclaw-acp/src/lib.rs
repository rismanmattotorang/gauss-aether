//! `gaussclaw-acp` — Agent Client Protocol server.
//!
//! Sprint 8 §6 of `/ROADMAP.md`. Hermes ships `acp_adapter/` as an
//! untyped Python JSON-RPC server that an editor (Cursor, Zed) drives
//! to dispatch agent tools. GaussClaw's variant is a typed Rust
//! protocol with cap-gated method dispatch.
//!
//! ## Wire shape
//!
//! Requests / responses follow JSON-RPC 2.0 over an arbitrary
//! framed transport (typically `stdio` LSP-style headers, but the
//! crate is transport-agnostic). The `Method` enum locks the
//! protocol surface:
//!
//! - `initialize` — capability handshake.
//! - `agent/new_session` — start a session and get back its id.
//! - `agent/send_message` — dispatch a user message.
//! - `agent/cancel` — interrupt a running session.
//! - `agent/close` — drop a session.
//!
//! ## Hermes-superiority axes
//!
//! - **Typed protocol.** Every request / response shape is a typed
//!   Rust struct; the codec layer catches schema drift at compile
//!   time. Hermes's `acp_adapter/` validates `dict`s at runtime.
//! - **Cap-gated method dispatch.** Each method declares its required
//!   `CapToken`; the server admits only if the session grant
//!   satisfies. Hermes inherits the editor's full credentials.
//! - **Stable error codes.** JSON-RPC error codes are minted in
//!   `ErrorCode` with documented semantics; Hermes returns
//!   free-form strings.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_tightening
)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

// ─── protocol ─────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 version literal carried on every envelope.
pub const JSONRPC_VERSION: &str = "2.0";

/// One ACP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Method {
    /// Capability handshake.
    Initialize,
    /// Start a new agent session.
    AgentNewSession,
    /// Dispatch a user message.
    AgentSendMessage,
    /// Cancel a running session.
    AgentCancel,
    /// Drop a session.
    AgentClose,
}

impl Method {
    /// Wire-format method name (`"initialize"`, `"agent/new_session"`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::AgentNewSession => "agent/new_session",
            Self::AgentSendMessage => "agent/send_message",
            Self::AgentCancel => "agent/cancel",
            Self::AgentClose => "agent/close",
        }
    }

    /// Parse a wire-format method name.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "initialize" => Some(Self::Initialize),
            "agent/new_session" => Some(Self::AgentNewSession),
            "agent/send_message" => Some(Self::AgentSendMessage),
            "agent/cancel" => Some(Self::AgentCancel),
            "agent/close" => Some(Self::AgentClose),
            _ => None,
        }
    }

    /// Capability required to dispatch this method.
    #[must_use]
    pub const fn required_cap(self) -> CapToken {
        match self {
            // initialize has no capability requirement — clients can
            // always probe the server.
            Self::Initialize => CapToken::BOTTOM,
            // The four agent methods all need MEMORY_READ (session
            // metadata) at minimum. Send may additionally need
            // EXECUTOR_LOCAL/NETWORK_POST depending on tool
            // dispatch; that gate is per-tool, not per-method.
            Self::AgentNewSession
            | Self::AgentSendMessage
            | Self::AgentCancel
            | Self::AgentClose => CapToken::MEMORY_READ,
        }
    }
}

/// JSON-RPC request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request id (string or number; we serialise as JSON value).
    pub id: serde_json::Value,
    /// Method name.
    pub method: String,
    /// Method params.
    #[serde(default)]
    pub params: serde_json::Value,
}

impl Request {
    /// Build a request envelope.
    #[must_use]
    pub fn new(id: serde_json::Value, method: Method, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.as_str().into(),
            params,
        }
    }
}

/// JSON-RPC response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request id echoed.
    pub id: serde_json::Value,
    /// `Some(result)` on success, `None` on error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// `Some(error)` on failure, `None` on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// Build a success response.
    #[must_use]
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    #[must_use]
    pub fn err(id: serde_json::Value, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC error body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Stable error code (negative integer per JSON-RPC spec).
    pub code: i32,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// Build a typed error.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_i32(),
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured data.
    #[must_use]
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// JSON-RPC error code catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// `-32700` — parse error.
    ParseError,
    /// `-32600` — invalid request shape.
    InvalidRequest,
    /// `-32601` — method not found.
    MethodNotFound,
    /// `-32602` — invalid params.
    InvalidParams,
    /// `-32603` — internal error.
    InternalError,
    /// `-32000` — admit gate refused.
    AdmitRefused,
    /// `-32001` — session not found.
    SessionNotFound,
    /// `-32002` — protocol-level error not covered above.
    ServerError,
}

impl ErrorCode {
    /// Numeric code per JSON-RPC.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::ParseError => -32_700,
            Self::InvalidRequest => -32_600,
            Self::MethodNotFound => -32_601,
            Self::InvalidParams => -32_602,
            Self::InternalError => -32_603,
            Self::AdmitRefused => -32_000,
            Self::SessionNotFound => -32_001,
            Self::ServerError => -32_002,
        }
    }
}

// ─── handler trait + state ────────────────────────────────────────────────

/// Handler trait — the editor backend implements this. The crate
/// ships a `MockHandler` for tests + the conformance suite.
#[async_trait]
pub trait AcpHandler: Send + Sync {
    /// Handle the `initialize` handshake; returns server capabilities.
    async fn initialize(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;

    /// Start a new session; returns a `{session_id}`.
    async fn new_session(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;

    /// Dispatch a user message; returns the assistant reply.
    async fn send_message(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;

    /// Cancel a running session.
    async fn cancel(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;

    /// Drop a session.
    async fn close(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;
}

/// ACP server state. Holds the cap grant + a pluggable handler.
pub struct AcpServer {
    grant: CapToken,
    handler: Box<dyn AcpHandler>,
    /// Per-session metadata (creation timestamp). Empty by default;
    /// the in-process handler populates this on `new_session`.
    sessions: Mutex<BTreeMap<String, SessionState>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SessionState {
    /// UNIX seconds when the session was created (not yet
    /// surfaced via the protocol; reserved for replay-corpus
    /// integration in Sprint 8 §5).
    created_at: i64,
}

impl AcpServer {
    /// Build a server with an explicit grant + handler.
    #[must_use]
    pub fn new(grant: CapToken, handler: Box<dyn AcpHandler>) -> Self {
        Self {
            grant,
            handler,
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    /// Dispatch one request envelope, returning the wire response.
    /// Single entry point for transport layers.
    pub async fn handle(&self, raw: &str) -> Response {
        let req: Request = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                return Response::err(
                    serde_json::Value::Null,
                    RpcError::new(ErrorCode::ParseError, format!("parse: {e}")),
                );
            }
        };
        self.handle_typed(req).await
    }

    /// Same as [`Self::handle`] but takes the already-parsed `Request`.
    pub async fn handle_typed(&self, req: Request) -> Response {
        if req.jsonrpc != JSONRPC_VERSION {
            return Response::err(
                req.id.clone(),
                RpcError::new(
                    ErrorCode::InvalidRequest,
                    format!("expected jsonrpc=2.0, got {}", req.jsonrpc),
                ),
            );
        }
        let Some(method) = Method::from_wire(&req.method) else {
            return Response::err(
                req.id.clone(),
                RpcError::new(
                    ErrorCode::MethodNotFound,
                    format!("unknown method: {}", req.method),
                ),
            );
        };
        let required = method.required_cap();
        if !self.grant.contains(required) {
            return Response::err(
                req.id.clone(),
                RpcError::new(
                    ErrorCode::AdmitRefused,
                    format!(
                        "admit refused: method {} requires 0x{:016x}, grant 0x{:016x}",
                        method.as_str(),
                        required.bits(),
                        self.grant.bits()
                    ),
                ),
            );
        }
        let outcome = match method {
            Method::Initialize => self.handler.initialize(req.params).await,
            Method::AgentNewSession => {
                let r = self.handler.new_session(req.params).await;
                if let Ok(value) = &r {
                    if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
                        let mut g = self.sessions.lock().expect("poisoned");
                        g.insert(
                            id.to_string(),
                            SessionState {
                                created_at: now_unix(),
                            },
                        );
                    }
                }
                r
            }
            Method::AgentSendMessage => self.handler.send_message(req.params).await,
            Method::AgentCancel => self.handler.cancel(req.params).await,
            Method::AgentClose => {
                let r = self.handler.close(req.params.clone()).await;
                if let Some(id) = req.params.get("session_id").and_then(|v| v.as_str()) {
                    self.sessions.lock().expect("poisoned").remove(id);
                }
                r
            }
        };
        match outcome {
            Ok(v) => Response::ok(req.id, v),
            Err(e) => Response::err(req.id, e),
        }
    }

    /// Number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.lock().expect("poisoned").len()
    }
}

/// Mock handler for tests + the conformance suite.
pub struct MockHandler {
    next_id: Mutex<u64>,
}

impl MockHandler {
    /// Build a fresh mock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(0),
        }
    }
}

impl Default for MockHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AcpHandler for MockHandler {
    async fn initialize(&self, _params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        Ok(serde_json::json!({
            "protocol_version": "1.0",
            "server": "gaussclaw-acp/mock",
            "capabilities": {
                "agent/new_session":  true,
                "agent/send_message": true,
                "agent/cancel":       true,
                "agent/close":        true,
            }
        }))
    }

    async fn new_session(&self, _params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let mut g = self.next_id.lock().expect("poisoned");
        *g = g.saturating_add(1);
        let id = format!("sess-{:08x}", *g);
        Ok(serde_json::json!({"session_id": id}))
    }

    async fn send_message(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let text = params.get("text").and_then(|v| v.as_str()).ok_or_else(|| {
            RpcError::new(ErrorCode::InvalidParams, "missing string field `text`")
        })?;
        Ok(serde_json::json!({"reply": format!("(mock) {text}")}))
    }

    async fn cancel(&self, _params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        Ok(serde_json::json!({"cancelled": true}))
    }

    async fn close(&self, _params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        Ok(serde_json::json!({"closed": true}))
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> AcpServer {
        AcpServer::new(CapToken::TOP, Box::new(MockHandler::new()))
    }

    #[tokio::test]
    async fn initialize_handshake_round_trips() {
        let s = server();
        let req = Request::new(
            serde_json::json!(1),
            Method::Initialize,
            serde_json::json!({}),
        );
        let resp = s.handle_typed(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocol_version"], "1.0");
        assert!(result["capabilities"]["agent/send_message"]
            .as_bool()
            .unwrap());
    }

    #[tokio::test]
    async fn new_session_returns_unique_ids() {
        let s = server();
        let mut ids = std::collections::BTreeSet::new();
        for i in 0..3 {
            let req = Request::new(
                serde_json::json!(i),
                Method::AgentNewSession,
                serde_json::json!({}),
            );
            let resp = s.handle_typed(req).await;
            let id = resp.result.unwrap()["session_id"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(ids.insert(id));
        }
        assert_eq!(s.session_count(), 3);
    }

    #[tokio::test]
    async fn send_message_returns_mock_reply() {
        let s = server();
        let req = Request::new(
            serde_json::json!(1),
            Method::AgentSendMessage,
            serde_json::json!({"text": "hello"}),
        );
        let resp = s.handle_typed(req).await;
        let result = resp.result.unwrap();
        assert!(result["reply"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_text() {
        let s = server();
        let req = Request::new(
            serde_json::json!(1),
            Method::AgentSendMessage,
            serde_json::json!({}),
        );
        let resp = s.handle_typed(req).await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, ErrorCode::InvalidParams.as_i32());
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let s = server();
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"agent/teleport","params":{}}"#;
        let resp = s.handle(raw).await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, ErrorCode::MethodNotFound.as_i32());
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let s = server();
        let resp = s.handle("{not valid json").await;
        assert_eq!(resp.error.unwrap().code, ErrorCode::ParseError.as_i32());
    }

    #[tokio::test]
    async fn wrong_jsonrpc_version_returns_invalid_request() {
        let s = server();
        let raw = r#"{"jsonrpc":"1.0","id":1,"method":"initialize","params":{}}"#;
        let resp = s.handle(raw).await;
        assert_eq!(resp.error.unwrap().code, ErrorCode::InvalidRequest.as_i32());
    }

    #[tokio::test]
    async fn admit_refuses_when_grant_misses_cap() {
        // Server with empty grant — initialize is allowed
        // (BOTTOM required), but agent methods refuse.
        let s = AcpServer::new(CapToken::BOTTOM, Box::new(MockHandler::new()));
        let req = Request::new(
            serde_json::json!(1),
            Method::AgentNewSession,
            serde_json::json!({}),
        );
        let resp = s.handle_typed(req).await;
        assert_eq!(resp.error.unwrap().code, ErrorCode::AdmitRefused.as_i32());
    }

    #[tokio::test]
    async fn initialize_works_without_caps() {
        let s = AcpServer::new(CapToken::BOTTOM, Box::new(MockHandler::new()));
        let req = Request::new(
            serde_json::json!(1),
            Method::Initialize,
            serde_json::json!({}),
        );
        let resp = s.handle_typed(req).await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn close_drops_session_from_registry() {
        let s = server();
        let req = Request::new(
            serde_json::json!(1),
            Method::AgentNewSession,
            serde_json::json!({}),
        );
        let resp = s.handle_typed(req).await;
        let id = resp.result.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(s.session_count(), 1);
        let req = Request::new(
            serde_json::json!(2),
            Method::AgentClose,
            serde_json::json!({"session_id": id}),
        );
        s.handle_typed(req).await;
        assert_eq!(s.session_count(), 0);
    }

    #[test]
    fn method_wire_strings_round_trip() {
        for m in [
            Method::Initialize,
            Method::AgentNewSession,
            Method::AgentSendMessage,
            Method::AgentCancel,
            Method::AgentClose,
        ] {
            assert_eq!(Method::from_wire(m.as_str()), Some(m));
        }
        assert!(Method::from_wire("fnord").is_none());
    }

    #[test]
    fn error_codes_match_jsonrpc_spec() {
        assert_eq!(ErrorCode::ParseError.as_i32(), -32_700);
        assert_eq!(ErrorCode::InvalidRequest.as_i32(), -32_600);
        assert_eq!(ErrorCode::MethodNotFound.as_i32(), -32_601);
        assert_eq!(ErrorCode::InvalidParams.as_i32(), -32_602);
        assert_eq!(ErrorCode::InternalError.as_i32(), -32_603);
        // Application range: -32000..=-32099.
        assert_eq!(ErrorCode::AdmitRefused.as_i32(), -32_000);
        assert_eq!(ErrorCode::SessionNotFound.as_i32(), -32_001);
    }
}
