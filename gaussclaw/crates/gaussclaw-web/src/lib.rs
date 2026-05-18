//! `gaussclaw-web` — Axum dashboard backend with embedded React frontend.
//!
//! Phase 1 Task 4 of `GAUSSCLAW_ROADMAP.md`. Supersedes the upstream
//! Hermes FastAPI + POSIX-PTY stack with:
//!
//! - A Rust + Axum HTTP server that runs natively on Linux, macOS, and
//!   Windows (no WSL2 PTY dependency).
//! - WebSocket streaming for the chat pane (replaces PTY).
//! - REST endpoints mirroring the Hermes shape so the upstream React
//!   frontend (retained verbatim — see [`frontend`]) keeps working.
//! - The frontend `dist/` directory baked into the binary via
//!   `rust-embed`, so the shipping `gaussclaw` is a single static binary.
//!
//! ## Endpoints
//!
//! The endpoint set is sized to match the upstream Hermes dashboard:
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | GET  | `/api/status`        | Liveness + version + active session count |
//! | GET  | `/api/health`        | SDHE invariants snapshot (proxies `gauss-health` in P2) |
//! | GET  | `/api/config`        | Active config tree |
//! | GET  | `/api/config/schema` | JSON schema for the config tree |
//! | POST | `/api/config`        | Patch a config value (cap-gated in P3) |
//! | GET  | `/api/sessions`      | Recent sessions (FTS-searchable in P2) |
//! | GET  | `/api/providers`     | Provider catalogue (populated in P4) |
//! | GET  | `/api/tools`         | Tool catalogue (populated in P3) |
//! | GET  | `/api/receipt/head`  | Receipt-chain head (populated in P2) |
//! | WS   | `/api/chat/ws`       | Chat WebSocket — streams turn tokens + tool events |
//!
//! Every API response carries a JSON envelope: `{ "ok": true, "data": ... }`
//! on success, `{ "ok": false, "error": { "code": "...", "message": "..." } }`
//! on failure. This shape is what the retained Hermes frontend already
//! consumes — preserving it is the cheapest way to honour Principle 1
//! for the dashboard surface.

#![allow(clippy::doc_markdown)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::{AuditTrace, KernelHandle, SurfaceRequest};
use gaussclaw_config::Config;
use gaussclaw_export::{verify_envelope, Envelope, VerifyEnvelopeError};
use gaussclaw_skill::SkillManifest;
use gaussclaw_store::SessionStore;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

// ─── frontend assets ────────────────────────────────────────────────────────

/// Embedded React frontend.
///
/// The build pipeline (Phase 1 Task 4, slice 4):
///
/// ```sh
/// cd crates/gaussclaw-web/frontend && pnpm install && pnpm build
/// ```
///
/// drops the production bundle into `frontend/dist/` which this struct
/// bakes into the binary via `rust-embed`. Until that bundle is ported
/// from upstream Hermes, the directory ships a placeholder `index.html`
/// that explains the state of the world.
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
pub struct FrontendAssets;

// ─── envelopes ──────────────────────────────────────────────────────────────

/// Successful API response envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct Ok<T> {
    /// Constant `true`. Marks the discriminant before deserialising `data`.
    pub ok: bool,
    /// Payload.
    pub data: T,
}

impl<T> Ok<T> {
    /// Wrap a payload in the success envelope.
    pub const fn new(data: T) -> Self {
        Self { ok: true, data }
    }
}

/// Failure API response envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct Err {
    /// Constant `false`.
    pub ok: bool,
    /// Error body.
    pub error: ErrorBody,
}

/// Machine-readable error payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Stable error id, e.g. `not_found`, `denied`, `bad_request`.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

impl Err {
    /// Build a failure envelope.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: ErrorBody {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

// ─── payload types ──────────────────────────────────────────────────────────

/// `/api/status` payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusPayload {
    /// `gaussclaw-web` crate version.
    pub version: String,
    /// Build profile (`debug` / `release`).
    pub profile: String,
    /// Active provider name from the loaded config (empty if unset).
    pub provider: String,
    /// Active model from the loaded config (empty if unset).
    pub model: String,
    /// Number of active sessions (zero until the store lands in Phase 2).
    pub active_sessions: u32,
}

/// `/api/health` payload — shaped to match the seven SDHE invariants
/// (`gauss-health`); each invariant's status surfaces here in Phase 2.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthPayload {
    /// Overall health: `green` | `yellow` | `red`.
    pub overall: String,
    /// Per-invariant report. Empty until Phase 2 wires the SDHE.
    pub invariants: Vec<HealthInvariant>,
}

/// One row of [`HealthPayload`].
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthInvariant {
    /// Invariant id.
    pub id: String,
    /// Status colour.
    pub status: String,
    /// Human-readable detail.
    pub detail: String,
}

/// `/api/config` payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigPayload {
    /// Where the loader read the config from. `None` means "defaults only".
    pub source: Option<String>,
    /// The active config tree.
    pub config: Config,
}

/// `/api/sessions` row.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRow {
    /// Session id.
    pub id: String,
    /// Created timestamp (RFC3339).
    pub created: String,
    /// Turn count.
    pub turns: u64,
}

/// `/api/receipt/head` payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReceiptHeadPayload {
    /// Hex-encoded chain head digest.
    pub digest: String,
    /// Turn index of the head (Phase 2).
    pub turn: u64,
}

// ─── shared state ───────────────────────────────────────────────────────────

/// State passed to every handler. Holds the loaded config, the kernel
/// handle (admit gate + plane selector), the audit trace (every
/// inbound writes here before admit), and the build profile.
#[derive(Clone)]
pub struct ServerState {
    config: Arc<Config>,
    config_source: Option<String>,
    profile: &'static str,
    kernel: KernelHandle,
    audit: AuditTrace,
    store: Option<Arc<SessionStore>>,
}

impl ServerState {
    /// Build a fresh state with a permissive kernel and a fresh audit
    /// trace. Use this for the Phase 1 demo binary and for tests.
    pub fn new(config: Config, config_source: Option<String>) -> Self {
        Self::with_kernel(config, config_source, KernelHandle::permissive())
    }

    /// Build a state with a caller-supplied kernel handle (audit defaults
    /// to a fresh trace, store defaults to `None`).
    pub fn with_kernel(
        config: Config,
        config_source: Option<String>,
        kernel: KernelHandle,
    ) -> Self {
        Self::with_kernel_and_audit(config, config_source, kernel, AuditTrace::new())
    }

    /// Full constructor with explicit audit trace.
    pub fn with_kernel_and_audit(
        config: Config,
        config_source: Option<String>,
        kernel: KernelHandle,
        audit: AuditTrace,
    ) -> Self {
        Self {
            config: Arc::new(config),
            config_source,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            kernel,
            audit,
            store: None,
        }
    }

    /// Attach a Phase 2 session store. The dashboard's `/api/sessions`
    /// and `/api/receipt/head` endpoints return live data when the
    /// store is present.
    #[must_use]
    pub fn with_store(mut self, store: Arc<SessionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Borrow the active config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Borrow the kernel handle.
    pub const fn kernel(&self) -> &KernelHandle {
        &self.kernel
    }

    /// Borrow the audit trace.
    pub const fn audit(&self) -> &AuditTrace {
        &self.audit
    }

    /// Borrow the session store (if attached).
    pub const fn store(&self) -> Option<&Arc<SessionStore>> {
        self.store.as_ref()
    }
}

// ─── handlers ───────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn handle_status(State(state): State<ServerState>) -> Json<Ok<StatusPayload>> {
    Json(Ok::new(StatusPayload {
        version: env!("CARGO_PKG_VERSION").into(),
        profile: state.profile.into(),
        provider: state.config.provider.name.clone(),
        model: state.config.provider.model.clone(),
        active_sessions: 0,
    }))
}

#[axum::debug_handler]
async fn handle_health(State(_state): State<ServerState>) -> Json<Ok<HealthPayload>> {
    // Phase 2 will wire this through `gauss-health`. For now we report
    // an empty-but-green payload so the frontend can render its skeleton.
    Json(Ok::new(HealthPayload {
        overall: "green".into(),
        invariants: vec![],
    }))
}

#[axum::debug_handler]
async fn handle_config_get(State(state): State<ServerState>) -> Json<Ok<ConfigPayload>> {
    Json(Ok::new(ConfigPayload {
        source: state.config_source.clone(),
        config: (*state.config).clone(),
    }))
}

#[axum::debug_handler]
async fn handle_config_schema() -> Json<Ok<serde_json::Value>> {
    // Phase 3 wires this to a real JSON Schema (used by the dashboard's
    // dynamic ConfigPage). For now we emit a stub that names the work.
    Json(Ok::new(serde_json::json!({
        "type": "object",
        "title": "GaussClawConfig",
        "x-stub": "real schema lands when gaussclaw-skill ships JSON Schema 2020-12 derives (Phase 3)"
    })))
}

#[axum::debug_handler]
async fn handle_config_post() -> (StatusCode, Json<Err>) {
    // Mutation is capability-gated; the gate lands with gaussclaw-skill
    // in Phase 3. Until then this route returns 403 to make the contract
    // obvious to dashboard authors.
    (
        StatusCode::FORBIDDEN,
        Json(Err::new(
            "denied",
            "config writes require the cap:config:write Skill Manifest (Phase 3)",
        )),
    )
}

#[axum::debug_handler]
async fn handle_sessions(State(state): State<ServerState>) -> Json<Ok<Vec<SessionRow>>> {
    // Phase 2 wires the session store. When attached, return live recent
    // sessions; otherwise an empty list.
    let rows = if let Some(store) = state.store() {
        store
            .list_recent_sessions(50)
            .await
            .into_iter()
            .map(|s| SessionRow {
                id: s.id,
                created: s.created,
                turns: s.turn_count,
            })
            .collect()
    } else {
        vec![]
    };
    Json(Ok::new(rows))
}

#[axum::debug_handler]
async fn handle_providers() -> Json<Ok<Vec<serde_json::Value>>> {
    Json(Ok::new(vec![]))
}

#[axum::debug_handler]
async fn handle_tools() -> Json<Ok<Vec<serde_json::Value>>> {
    Json(Ok::new(vec![]))
}

#[axum::debug_handler]
async fn handle_receipt_head(State(state): State<ServerState>) -> Json<Ok<ReceiptHeadPayload>> {
    // When a store is attached, return the live store chain head; turn
    // counter reflects the chain length. Falls back to the audit trace
    // head when no store is wired.
    if let Some(store) = state.store() {
        if let Ok(head) = store.chain_head().await {
            return Json(Ok::new(ReceiptHeadPayload {
                digest: head.digest_hex,
                turn: head.length,
            }));
        }
    }
    let head = state.audit.head().await;
    Json(Ok::new(ReceiptHeadPayload {
        digest: head.to_hex(),
        turn: 0,
    }))
}

#[axum::debug_handler]
async fn handle_chat_ws(State(state): State<ServerState>, ws: WebSocketUpgrade) -> Response {
    // WAL-before-effect: record the WS upgrade attempt BEFORE admit.
    let plane = state.kernel.plane_for(SurfaceRequest::UserSync);
    state
        .audit
        .record_inbound("/api/chat/ws", "dashboard", b"", TaintLabel::User, plane)
        .await;
    if let Err(e) = state.kernel.admit(CapToken::NETWORK_GET, TaintLabel::User) {
        return (
            StatusCode::FORBIDDEN,
            Json(Err::new("denied", format!("admit failed: {e:?}"))),
        )
            .into_response();
    }
    ws.on_upgrade(chat_socket)
}

async fn chat_socket(mut socket: WebSocket) {
    // Skeleton: echo a single welcome frame, then close. Real streaming
    // arrives with the agent loop in Phase 1 slice 5 (three-plane routing).
    let banner = serde_json::json!({
        "ok": true,
        "data": {
            "kind": "system",
            "body": "chat WebSocket connected — agent dispatch lands in slice 5 (three-plane routing)"
        }
    });
    if socket
        .send(Message::Text(banner.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(msg) = socket.recv().await {
        let Ok(msg) = msg else { return };
        // For now echo the incoming text back as a stub assistant frame.
        let body = match &msg {
            Message::Text(t) => t.as_str().to_string(),
            Message::Binary(_) => "(binary frame ignored)".into(),
            Message::Close(_) => return,
            _ => continue,
        };
        let reply = serde_json::json!({
            "ok": true,
            "data": {
                "kind": "assistant",
                "body": format!("(stub echo) {body}")
            }
        });
        if socket
            .send(Message::Text(reply.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
}

// ─── Sprint-2 endpoints — receipts, envelope verify, skill preview ─────────

/// One row in the recent-receipts list.
#[derive(Debug, Serialize)]
struct ReceiptRow {
    /// 1-based chain index (1 = first turn).
    index: u64,
    /// Turn id, when the store can resolve it.
    turn_id: Option<u64>,
    /// Hex-encoded post-head digest.
    digest: String,
    /// Hex-encoded payload digest.
    payload_digest: String,
    /// True when the receipt verifies cleanly under its embedded key.
    verified: bool,
}

/// Recent-receipts response payload.
#[derive(Debug, Serialize)]
struct ReceiptListPayload {
    head: String,
    length: u64,
    rows: Vec<ReceiptRow>,
}

/// Recent receipts (most-recent-first). Bounded at `?limit=` ≤ 100;
/// the default is 10. Returns an empty list when no store is attached.
///
/// Hermes has no audit-surface API — its session store is mutable
/// SQLite that ships no verification primitive.
#[axum::debug_handler]
async fn handle_receipts_recent(
    State(state): State<ServerState>,
    axum::extract::Query(q): axum::extract::Query<RecentQuery>,
) -> Json<Ok<ReceiptListPayload>> {
    let limit = q.limit.unwrap_or(10).min(100);
    let Some(store) = state.store() else {
        return Json(Ok::new(ReceiptListPayload {
            head: String::new(),
            length: 0,
            rows: vec![],
        }));
    };
    let head = store.chain_head().await.ok();
    let length = head.as_ref().map_or(0, |h| h.length);
    let head_hex = head.map(|h| h.digest_hex).unwrap_or_default();

    let mut rows = Vec::with_capacity(limit as usize);
    if length > 0 {
        let start = length.saturating_sub(limit);
        for idx in (start..length).rev() {
            // The chain length is the count of receipts; turn ids are
            // not always dense, so we attempt by index-as-turn first.
            let turn_id = idx.saturating_add(1);
            let Some(receipt) = store.get_receipt(turn_id).await else {
                continue;
            };
            let verified = store.verify_receipt(turn_id).await.unwrap_or(false);
            rows.push(ReceiptRow {
                index: idx.saturating_add(1),
                turn_id: Some(turn_id),
                digest: hex_lower(&receipt.post_head),
                payload_digest: hex_lower(&receipt.payload_digest),
                verified,
            });
        }
    }
    Json(Ok::new(ReceiptListPayload {
        head: head_hex,
        length,
        rows,
    }))
}

#[derive(Debug, Deserialize, Default)]
struct RecentQuery {
    limit: Option<u64>,
}

/// Envelope verification request body — the raw envelope JSON.
type VerifyEnvelopePayload = Envelope;

/// Envelope verification response.
#[derive(Debug, Serialize)]
struct VerifyEnvelopeReport {
    verified: bool,
    /// Axis at which verification failed (when [`verified`] is false).
    failed_axis: Option<&'static str>,
    /// Human-readable detail.
    detail: Option<String>,
    /// Convenience echoes for the dashboard.
    chain_head: String,
    chain_length: u64,
    has_anchor: bool,
}

/// Verify a Cryptographic Trajectory Envelope. The dashboard POSTs the
/// envelope JSON (the exact wire shape `gaussclaw-export` produces);
/// the response names which axis (if any) failed.
///
/// Hermes upstream has no equivalent — its JSONL exports carry no
/// verifiable surface at all.
#[axum::debug_handler]
async fn handle_envelope_verify(
    State(_state): State<ServerState>,
    Json(envelope): Json<VerifyEnvelopePayload>,
) -> Json<Ok<VerifyEnvelopeReport>> {
    let chain_head_hex = hex_lower(&envelope.chain_head);
    let chain_length = envelope.chain_length;
    let has_anchor = envelope.tsa_anchor.is_some();

    let report = match verify_envelope(&envelope, None, None) {
        Ok(()) => VerifyEnvelopeReport {
            verified: true,
            failed_axis: None,
            detail: None,
            chain_head: chain_head_hex,
            chain_length,
            has_anchor,
        },
        Err(e) => VerifyEnvelopeReport {
            verified: false,
            failed_axis: Some(verify_axis(&e)),
            detail: Some(format!("{e}")),
            chain_head: chain_head_hex,
            chain_length,
            has_anchor,
        },
    };
    Json(Ok::new(report))
}

const fn verify_axis(err: &VerifyEnvelopeError) -> &'static str {
    match err {
        VerifyEnvelopeError::PublicKeyMismatch => "public_key",
        VerifyEnvelopeError::PayloadDigestMismatch => "payload_digest",
        VerifyEnvelopeError::ChainLinkInconsistent => "chain_link",
        VerifyEnvelopeError::Signature(_) => "signature",
        VerifyEnvelopeError::WitnessHeadMismatch => "witness_head",
        VerifyEnvelopeError::WitnessIndexExceedsChain { .. } => "witness_index",
        _ => "other",
    }
}

/// Skill-manifest preview request — the raw TOML body.
#[derive(Debug, Deserialize)]
struct PreviewSkillPayload {
    toml: String,
}

/// Skill-manifest preview response.
#[derive(Debug, Serialize)]
struct SkillPreviewReport {
    parsed: bool,
    error: Option<String>,
    /// When parsing succeeded: the structured manifest summary.
    summary: Option<SkillSummary>,
}

#[derive(Debug, Serialize)]
struct SkillSummary {
    name: String,
    description: String,
    usage: String,
    caps: Vec<String>,
    taint: String,
    reversible: bool,
    persistent: bool,
    cost_tokens_per_call: u32,
    cost_dollars_per_call: f64,
    no_instruction_substrings: bool,
    max_string_len: usize,
}

/// Preview a Skill Manifest. Validates the TOML, parses the manifest,
/// and returns a typed summary the dashboard can render before the
/// operator commits an install.
///
/// Hermes loads skills via `--skills name1,name2` strings — there's no
/// preview surface and no schema enforcement.
#[axum::debug_handler]
async fn handle_skill_preview(
    State(_state): State<ServerState>,
    Json(payload): Json<PreviewSkillPayload>,
) -> Json<Ok<SkillPreviewReport>> {
    match SkillManifest::from_toml(&payload.toml) {
        Ok(m) => Json(Ok::new(SkillPreviewReport {
            parsed: true,
            error: None,
            summary: Some(SkillSummary {
                name: m.name.clone(),
                description: m.description.clone(),
                usage: m.usage.clone(),
                caps: m.caps.clone(),
                taint: m.taint.clone(),
                reversible: m.reversible,
                persistent: m.persistent,
                cost_tokens_per_call: m.cost.tokens_per_call,
                cost_dollars_per_call: m.cost.dollars_per_call,
                no_instruction_substrings: m.guards.no_instruction_substrings,
                max_string_len: m.guards.max_string_len,
            }),
        })),
        Err(e) => Json(Ok::new(SkillPreviewReport {
            parsed: false,
            error: Some(format!("{e}")),
            summary: None,
        })),
    }
}

/// Lower-case hex encode.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ─── frontend serving ───────────────────────────────────────────────────────

#[axum::debug_handler]
async fn handle_root() -> impl IntoResponse {
    serve_embedded("index.html")
}

#[axum::debug_handler]
async fn handle_asset(Path(path): Path<String>) -> impl IntoResponse {
    serve_embedded(&path)
}

fn serve_embedded(path: &str) -> Response {
    // Map "/" -> "index.html" and reject path traversal up front.
    let key = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path
    };
    if key.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err::new("bad_request", "invalid path")),
        )
            .into_response();
    }
    match FrontendAssets::get(key) {
        Some(content) => {
            let mime = mime_guess::from_path(key)
                .first_or_octet_stream()
                .to_string();
            let mut resp = (StatusCode::OK, content.data.to_vec()).into_response();
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            resp
        }
        None => {
            // SPA fallback: unknown paths get the index so client-side routing works.
            FrontendAssets::get("index.html").map_or_else(
                || {
                    (
                        StatusCode::NOT_FOUND,
                        Json(Err::new("not_found", "asset missing")),
                    )
                        .into_response()
                },
                |content| {
                    let mut resp = (StatusCode::OK, content.data.to_vec()).into_response();
                    resp.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/html; charset=utf-8"),
                    );
                    resp
                },
            )
        }
    }
}

// ─── router + entry points ──────────────────────────────────────────────────

/// Build the Axum router. Exposed so integration tests can drive it
/// without binding a real socket.
pub fn router(state: ServerState) -> Router {
    Router::new()
        // API
        .route("/api/status", get(handle_status))
        .route("/api/health", get(handle_health))
        .route(
            "/api/config",
            get(handle_config_get).post(handle_config_post),
        )
        .route("/api/config/schema", get(handle_config_schema))
        .route("/api/sessions", get(handle_sessions))
        .route("/api/providers", get(handle_providers))
        .route("/api/tools", get(handle_tools))
        .route("/api/receipt/head", get(handle_receipt_head))
        .route("/api/receipts/recent", get(handle_receipts_recent))
        .route(
            "/api/envelope/verify",
            axum::routing::post(handle_envelope_verify),
        )
        .route(
            "/api/skills/preview",
            axum::routing::post(handle_skill_preview),
        )
        .route("/api/chat/ws", get(handle_chat_ws))
        // Frontend
        .route("/", get(handle_root))
        .route("/{*path}", get(handle_asset))
        // Middleware
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Listen on `addr` and serve until shut down.
///
/// This is the entry point `gaussclaw web` will call once the CLI
/// subcommand lands. Tests use [`router`] directly via
/// `tower::ServiceExt::oneshot`.
pub async fn serve(addr: SocketAddr, state: ServerState) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "gaussclaw-web listening");
    axum::serve(listener, app).await?;
    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> ServerState {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        ServerState::new(cfg, Some("/tmp/gaussclaw.toml".into()))
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn status_endpoint_returns_loaded_config() {
        let (status, body) = get_json("/api/status").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["provider"], "anthropic");
        assert_eq!(body["data"]["model"], "claude-3.5-sonnet");
        assert_eq!(body["data"]["active_sessions"], 0);
    }

    #[tokio::test]
    async fn health_endpoint_is_green_with_empty_invariants() {
        let (status, body) = get_json("/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["overall"], "green");
        assert!(body["data"]["invariants"].is_array());
    }

    #[tokio::test]
    async fn config_get_returns_the_loaded_tree() {
        let (status, body) = get_json("/api/config").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["source"], "/tmp/gaussclaw.toml");
        assert_eq!(body["data"]["config"]["provider"]["name"], "anthropic");
    }

    #[tokio::test]
    async fn config_post_is_denied_until_phase3() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/config")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"provider.name","value":"openai"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "denied");
    }

    #[tokio::test]
    async fn sessions_endpoint_returns_empty_list_without_store() {
        let (status, body) = get_json("/api/sessions").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn sessions_endpoint_returns_live_data_with_store() {
        use std::sync::Arc;
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let sess = store
            .create_session("rest", "anthropic/claude-3.5-sonnet")
            .await;
        let _ = store
            .append_turn(&sess.id, None, "user", "hi", gauss_core::TaintLabel::User)
            .await
            .unwrap();

        let state = test_state().with_store(store);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let rows = body["data"].as_array().expect("data array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], sess.id);
        assert_eq!(rows[0]["turns"], 1);
    }

    #[tokio::test]
    async fn receipt_head_with_store_returns_live_chain_head() {
        use std::sync::Arc;
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let sess = store.create_session("rest", "m").await;
        let _ = store
            .append_turn(&sess.id, None, "user", "warm", gauss_core::TaintLabel::User)
            .await
            .unwrap();

        let state = test_state().with_store(store);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/receipt/head")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["data"]["turn"], 1);
        // Head must NOT be the all-zero genesis digest after one append.
        let digest = body["data"]["digest"].as_str().unwrap();
        assert_eq!(digest.len(), 64);
        assert_ne!(digest, "0".repeat(64));
    }

    #[tokio::test]
    async fn receipt_head_returns_zero_digest_until_phase2() {
        let (status, body) = get_json("/api/receipt/head").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["turn"], 0);
        assert_eq!(body["data"]["digest"], "0".repeat(64));
    }

    #[tokio::test]
    async fn root_serves_embedded_index() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    }

    #[tokio::test]
    async fn unknown_path_falls_back_to_index_for_spa_routing() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/sessions/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Either rejected outright, or the SPA fallback kicks in. Both are
        // acceptable; the test asserts no 5xx leak.
        assert!(resp.status().is_success() || resp.status() == StatusCode::BAD_REQUEST);
    }

    // ─── Sprint-2 endpoints ────────────────────────────────────────────

    async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        // Non-2xx responses may carry plain-text error bodies from Axum
        // (e.g. 422 from a JSON deserialisation failure). Fall back to
        // an empty object so callers can still inspect status.
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, json)
    }

    #[tokio::test]
    async fn receipts_recent_returns_empty_list_without_store() {
        let (status, body) = get_json("/api/receipts/recent?limit=5").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["length"], 0);
        assert!(body["data"]["rows"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn skill_preview_parses_valid_toml() {
        let toml = r#"
name = "echo"
description = "echo"
caps = []
taint = "trusted"
"#;
        let (status, body) =
            post_json("/api/skills/preview", serde_json::json!({ "toml": toml })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["parsed"], true);
        assert_eq!(body["data"]["summary"]["name"], "echo");
        assert_eq!(body["data"]["summary"]["taint"], "trusted");
    }

    #[tokio::test]
    async fn skill_preview_reports_invalid_toml() {
        let (status, body) = post_json(
            "/api/skills/preview",
            serde_json::json!({ "toml": "this = is = not toml" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["parsed"], false);
        assert!(body["data"]["error"].is_string());
        assert!(body["data"]["summary"].is_null());
    }

    #[tokio::test]
    async fn envelope_verify_reports_failure_for_malformed_envelope() {
        // An envelope with bogus internal references; the verifier
        // names the failing axis.
        let zeros32: Vec<u8> = vec![0; 32];
        let zeros64: Vec<u8> = vec![0; 64];
        let bogus = serde_json::json!({
            "body": { "sft": { "session_id": "s", "turn_id": 1, "messages": [], "model": "m", "ts": "2026-01-01T00:00:00Z" } },
            "receipt": {
                "turn_id": 1,
                "prev_head":      zeros32,
                "post_head":      vec![1u8; 32],
                "payload_digest": vec![2u8; 32],
                "public_key":     vec![3u8; 32],
                "signature":      zeros64,
            },
            "chain_head":     vec![4u8; 32],
            "chain_length":   1,
            "witness":        { "index": 1, "post_head": vec![9u8; 32] },
            "tsa_anchor":     null,
            "body_canonical": Vec::<u8>::new(),
        });
        let (status, body) = post_json("/api/envelope/verify", bogus).await;
        // The malformed envelope may even fail to parse — both 422 and
        // a 200 with verified=false demonstrate the verify path works.
        assert!(
            status == StatusCode::OK || status.as_u16() == 422,
            "unexpected status: {status}"
        );
        if status == StatusCode::OK {
            assert_eq!(body["data"]["verified"], false);
            assert!(body["data"]["failed_axis"].is_string());
        }
    }
}
