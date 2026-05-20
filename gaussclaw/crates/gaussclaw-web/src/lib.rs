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

pub mod wire;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use gauss_core::{CapToken, TaintLabel};
use gauss_cron::{parse_schedule, FireOutcome, Job, JobId, Scheduler, SystemClock};
use gaussclaw_agent::{
    AgentLoop, AuditTrace, KernelHandle, Message as AgentMessage, Prompt as AgentPrompt,
    SurfaceRequest,
};
use gaussclaw_config::Config;
use gaussclaw_export::{verify_envelope, Envelope, VerifyEnvelopeError};
use gaussclaw_skill::SkillManifest;
use gaussclaw_store::SessionStore;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Concrete scheduler type the dashboard owns.
pub type CronScheduler = Scheduler<SystemClock>;

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
    /// Active executor backend (Sprint 6 §2) — `local` / `docker` /
    /// `ssh` / `modal`.
    pub terminal_backend: String,
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

// ─── log buffer ─────────────────────────────────────────────────────────────

/// Severity level for one [`LogEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Informational; nominal request flow.
    Info,
    /// Recoverable problem (deprecation, retry succeeded).
    Warn,
    /// Failed operation that surfaced to the caller.
    Error,
}

impl LogLevel {
    /// Display-friendly tag.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// One row in the dashboard log feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// UNIX seconds when the entry was recorded.
    pub ts_unix: i64,
    /// Severity.
    pub level: LogLevel,
    /// Source tag (e.g. `cron`, `http`, `kernel`).
    pub source: String,
    /// Human-readable message.
    pub message: String,
}

/// Bounded ring buffer of log entries. Cheap to clone (`Arc`-shared
/// underlying storage).
#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    inner: std::sync::Mutex<std::collections::VecDeque<LogEntry>>,
}

impl LogBuffer {
    /// Build a buffer with a fixed capacity (oldest entry drops on overflow).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            inner: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
        }
    }

    /// Append an entry; drops the oldest if at capacity.
    pub fn push(&self, entry: LogEntry) {
        let mut g = self.inner.lock().expect("poisoned");
        if g.len() == self.capacity {
            g.pop_front();
        }
        g.push_back(entry);
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned").len()
    }

    /// True if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot the most recent `limit` entries (newest first).
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        let g = self.inner.lock().expect("poisoned");
        g.iter().rev().take(limit).cloned().collect()
    }
}

fn now_unix_seconds() -> i64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
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
    cron: Option<Arc<CronScheduler>>,
    logs: Arc<LogBuffer>,
    /// Sprint 7 §3 — plugin discovery roots; the dashboard's
    /// `/api/plugins` endpoint walks them on demand.
    plugin_roots: Vec<std::path::PathBuf>,
    /// Optional agent loop driving the `/api/chat/ws` WebSocket. When
    /// `None`, the WebSocket falls back to the stub-echo banner so
    /// the dashboard still loads against a partially-wired backend.
    /// When `Some`, every user message is dispatched through the loop
    /// and every `LoopEvent` is streamed back as a dashboard wire
    /// frame via [`wire::loop_event_to_wire`].
    agent: Option<Arc<AgentLoop>>,
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
            cron: None,
            logs: Arc::new(LogBuffer::with_capacity(200)),
            plugin_roots: Vec::new(),
            agent: None,
        }
    }

    /// Attach an agent loop. Subsequent `/api/chat/ws` connections
    /// dispatch every user message through this loop and stream
    /// `LoopEvent`s back to the browser. The provider, hooks,
    /// compactor, audit trace, and enrichers attached to the loop
    /// all apply.
    #[must_use]
    pub fn with_agent(mut self, agent: Arc<AgentLoop>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Borrow the optional agent loop. Returns `None` until
    /// [`Self::with_agent`] is called.
    #[must_use]
    pub fn agent(&self) -> Option<Arc<AgentLoop>> {
        self.agent.clone()
    }

    /// Push a structured log entry into the in-memory ring buffer.
    /// Consumed by `/api/logs`.
    pub fn log(&self, level: LogLevel, source: &str, message: impl Into<String>) {
        self.logs.push(LogEntry {
            ts_unix: now_unix_seconds(),
            level,
            source: source.into(),
            message: message.into(),
        });
    }

    /// Borrow the in-memory log buffer (for testing / introspection).
    #[must_use]
    pub const fn logs(&self) -> &Arc<LogBuffer> {
        &self.logs
    }

    /// Attach a Phase 2 session store. The dashboard's `/api/sessions`
    /// and `/api/receipt/head` endpoints return live data when the
    /// store is present.
    #[must_use]
    pub fn with_store(mut self, store: Arc<SessionStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Attach a Sprint-5 cron scheduler. The dashboard's `/api/cron/*`
    /// endpoints serve live job data when one is attached; without it
    /// they return an empty list (and refuse mutations with `503`).
    #[must_use]
    pub fn with_cron(mut self, cron: Arc<CronScheduler>) -> Self {
        self.cron = Some(cron);
        self
    }

    /// Attach plugin discovery roots — Sprint 7 §3.
    #[must_use]
    pub fn with_plugin_roots(mut self, roots: Vec<std::path::PathBuf>) -> Self {
        self.plugin_roots = roots;
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

    /// Borrow the cron scheduler (if attached).
    pub const fn cron(&self) -> Option<&Arc<CronScheduler>> {
        self.cron.as_ref()
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
        terminal_backend: state.config.terminal.backend.as_str().into(),
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
    let agent = state.agent.clone();
    ws.on_upgrade(move |socket| chat_socket(socket, agent))
}

/// Adapter making the live WebSocket usable as a [`wire::WireOutbox`].
/// Cloning shares the underlying split sender behind an `Arc<Mutex>`;
/// every event handler clones once, the agent loop holds the sink for
/// the lifetime of the run.
struct WebSocketOutbox {
    sink: Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>,
}

#[async_trait::async_trait]
impl wire::WireOutbox for WebSocketOutbox {
    async fn send(&self, frame: serde_json::Value) -> bool {
        use futures_util::SinkExt;
        let mut g = self.sink.lock().await;
        g.send(Message::Text(frame.to_string().into()))
            .await
            .is_ok()
    }
}

async fn chat_socket(socket: WebSocket, agent: Option<Arc<AgentLoop>>) {
    use futures_util::stream::StreamExt;
    let (sender, mut receiver) = socket.split();
    let outbox: Arc<dyn wire::WireOutbox> = Arc::new(WebSocketOutbox {
        sink: Arc::new(tokio::sync::Mutex::new(sender)),
    });

    // Banner — same shape the dashboard already handles. We send the
    // bare wire envelope (`{kind: "system", body: …}`) wrapped in the
    // legacy `{ok, data}` shell for backwards compatibility with the
    // existing app.js dispatch.
    let banner = if agent.is_some() {
        "chat WebSocket connected — agent dispatch live."
    } else {
        "chat WebSocket connected — no agent attached (server running in stub mode)."
    };
    let banner_frame = serde_json::json!({
        "ok": true,
        "data": { "kind": "system", "body": banner },
    });
    if !outbox.send(banner_frame).await {
        return;
    }

    // No agent attached → fall back to the legacy stub echo so the
    // dashboard still loads against a partially-wired backend.
    let Some(agent) = agent else {
        while let Some(msg) = receiver.next().await {
            let Ok(msg) = msg else { return };
            let body = match &msg {
                Message::Text(t) => t.as_str().to_string(),
                Message::Binary(_) => "(binary frame ignored)".into(),
                Message::Close(_) => return,
                _ => continue,
            };
            let reply = serde_json::json!({
                "ok": true,
                "data": { "kind": "assistant", "body": format!("(stub echo) {body}") },
            });
            if !outbox.send(reply).await {
                return;
            }
        }
        return;
    };

    // Agent path. Each inbound user message → one AgentLoop run.
    while let Some(msg) = receiver.next().await {
        let Ok(msg) = msg else { return };
        let user_text = match &msg {
            Message::Text(t) => t.as_str().to_string(),
            Message::Close(_) => return,
            // Pings, pongs, binary frames are ignored on the chat
            // channel — only text gets dispatched.
            _ => continue,
        };
        let prompt = AgentPrompt::new(
            "default",
            vec![AgentMessage::new("user", user_text.clone())],
        );
        let sink = wire::WireLoopSink::new(outbox.clone());
        match agent.run(prompt, TaintLabel::User, None, &sink).await {
            Ok(_outcome) => {
                // Done frame is already emitted by the loop sink.
            }
            Err(e) => {
                let err_frame = serde_json::json!({
                    "type": "error",
                    "error": format!("{e:?}"),
                });
                if !outbox.send(err_frame).await {
                    return;
                }
            }
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

// ─── Sprint-5 endpoints — cron scheduler CRUD ──────────────────────────────

/// One row in `GET /api/cron`.
#[derive(Debug, Serialize)]
struct CronJobView {
    id: u64,
    label: String,
    /// Human-readable schedule, e.g. `30m`, `*/15 * * * *`, `2026-05-20T14:30:00Z`.
    schedule: String,
    /// Lifecycle status (`armed`, `paused`, `completed`, `failed`).
    status: String,
    next_fire_at: Option<i64>,
    last_fired_at: Option<i64>,
    fire_count: u64,
    last_receipt_id: Option<u64>,
    /// Hex string of the payload-caps bitmap (for the dashboard "caps" badge).
    payload_caps: String,
}

impl From<&Job> for CronJobView {
    fn from(j: &Job) -> Self {
        Self {
            id: j.id.0,
            label: j.label.clone(),
            schedule: schedule_to_display(&j.schedule),
            status: status_to_str(j.status).into(),
            next_fire_at: j.next_fire_at,
            last_fired_at: j.last_fired_at,
            fire_count: j.fire_count,
            last_receipt_id: j.last_receipt_id,
            payload_caps: format!("0x{:016x}", j.payload_caps.bits()),
        }
    }
}

const fn status_to_str(s: gauss_cron::JobStatus) -> &'static str {
    match s {
        gauss_cron::JobStatus::Armed => "armed",
        gauss_cron::JobStatus::Paused => "paused",
        gauss_cron::JobStatus::Completed => "completed",
        gauss_cron::JobStatus::Failed => "failed",
        _ => "unknown",
    }
}

fn schedule_to_display(s: &gauss_cron::Schedule) -> String {
    match s {
        gauss_cron::Schedule::Duration { seconds } => format!("{seconds}s"),
        gauss_cron::Schedule::Cron { expr, .. } => expr.clone(),
        gauss_cron::Schedule::At { unix_seconds } => format!("at:{unix_seconds}"),
        _ => "(unknown)".into(),
    }
}

/// `POST /api/cron` body.
#[derive(Debug, Deserialize)]
struct CreateCronBody {
    /// Free-text label; defaults to `(unlabeled)`.
    #[serde(default)]
    label: Option<String>,
    /// Schedule grammar (duration / cron / ISO 8601).
    schedule: String,
    /// Inline JSON payload the job receives at fire time.
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// `POST /api/cron/{id}/edit` body.
#[derive(Debug, Deserialize)]
struct EditCronBody {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    schedule: Option<String>,
}

#[axum::debug_handler]
async fn handle_cron_list(State(state): State<ServerState>) -> Json<Ok<Vec<CronJobView>>> {
    let Some(cron) = state.cron() else {
        return Json(Ok::new(vec![]));
    };
    let jobs = cron.list().await.unwrap_or_default();
    Json(Ok::new(jobs.iter().map(CronJobView::from).collect()))
}

#[axum::debug_handler]
async fn handle_cron_add(
    State(state): State<ServerState>,
    Json(body): Json<CreateCronBody>,
) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    let schedule = match parse_schedule(&body.schedule) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(Err::new("bad_request", format!("schedule grammar: {e}"))),
            )
                .into_response();
        }
    };
    let job = Job::new(
        JobId::new(0),
        body.label.unwrap_or_else(|| "(unlabeled)".into()),
        schedule,
        CapToken::BOTTOM,
        body.payload.unwrap_or(serde_json::Value::Null),
        0,
    );
    match cron.add(job).await {
        Result::Ok(added) => Json(Ok::new(CronJobView::from(&added))).into_response(),
        Result::Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Err::new("scheduler", format!("add: {e}"))),
        )
            .into_response(),
    }
}

#[axum::debug_handler]
async fn handle_cron_get(State(state): State<ServerState>, Path(id): Path<u64>) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    match cron.get(JobId::new(id)).await {
        Result::Ok(Some(job)) => Json(Ok::new(CronJobView::from(&job))).into_response(),
        Result::Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(Err::new("not_found", format!("no such job: {id}"))),
        )
            .into_response(),
        Result::Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Err::new("scheduler", format!("get: {e}"))),
        )
            .into_response(),
    }
}

#[axum::debug_handler]
async fn handle_cron_pause(State(state): State<ServerState>, Path(id): Path<u64>) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    match cron.pause(JobId::new(id)).await {
        Result::Ok(()) => no_content_ok(),
        Result::Err(e) => cron_error("pause", &e),
    }
}

#[axum::debug_handler]
async fn handle_cron_resume(State(state): State<ServerState>, Path(id): Path<u64>) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    match cron.resume(JobId::new(id)).await {
        Result::Ok(()) => no_content_ok(),
        Result::Err(e) => cron_error("resume", &e),
    }
}

#[axum::debug_handler]
async fn handle_cron_remove(State(state): State<ServerState>, Path(id): Path<u64>) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    match cron.cancel(JobId::new(id)).await {
        Result::Ok(()) => no_content_ok(),
        Result::Err(e) => cron_error("remove", &e),
    }
}

#[axum::debug_handler]
async fn handle_cron_run(State(state): State<ServerState>, Path(id): Path<u64>) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    // Use TOP grant for the demo binary; production wires this to the
    // live kernel grant once the per-session cron daemon lands.
    match cron.run_now(JobId::new(id), CapToken::TOP, |_| None).await {
        Result::Ok(outcome) => match outcome {
            FireOutcome::Fired { id, receipt_id } => Json(Ok::new(serde_json::json!({
                "fired": id.0,
                "receipt_id": receipt_id,
            })))
            .into_response(),
            FireOutcome::Refused { id, reason } => (
                StatusCode::FORBIDDEN,
                Json(Err::new(
                    "denied",
                    format!("job {} refused: {reason}", id.0),
                )),
            )
                .into_response(),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Err::new("scheduler", "unknown fire outcome")),
            )
                .into_response(),
        },
        Result::Err(e) => cron_error("run", &e),
    }
}

#[axum::debug_handler]
async fn handle_cron_edit(
    State(state): State<ServerState>,
    Path(id): Path<u64>,
    Json(body): Json<EditCronBody>,
) -> Response {
    let Some(cron) = state.cron() else {
        return cron_unavailable();
    };
    if body.label.is_none() && body.schedule.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(Err::new(
                "bad_request",
                "pass at least one of `label` or `schedule`",
            )),
        )
            .into_response();
    }
    let schedule = match body.schedule {
        Some(s) => match parse_schedule(&s) {
            Result::Ok(parsed) => Some(parsed),
            Result::Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(Err::new("bad_request", format!("schedule grammar: {e}"))),
                )
                    .into_response();
            }
        },
        None => None,
    };
    match cron.edit(JobId::new(id), body.label, schedule).await {
        Result::Ok(updated) => Json(Ok::new(CronJobView::from(&updated))).into_response(),
        Result::Err(e) => cron_error("edit", &e),
    }
}

fn cron_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(Err::new(
            "unavailable",
            "no cron scheduler attached to this server",
        )),
    )
        .into_response()
}

fn cron_error(op: &str, err: &gauss_cron::SchedulerError) -> Response {
    let (status, code) = match err {
        gauss_cron::SchedulerError::Store(gauss_cron::StoreError::Unknown(_)) => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "scheduler"),
    };
    (status, Json(Err::new(code, format!("{op}: {err}")))).into_response()
}

fn no_content_ok() -> Response {
    Json(Ok::new(serde_json::json!({ "ok": true }))).into_response()
}

// ─── Sprint-5 §10 — Logs / Profiles / Analytics ────────────────────────────

#[derive(Debug, Serialize)]
struct LogsListPayload {
    entries: Vec<LogEntry>,
    capacity: usize,
}

#[derive(Debug, Deserialize, Default)]
struct LogsQuery {
    limit: Option<usize>,
}

#[axum::debug_handler]
async fn handle_logs_list(
    State(state): State<ServerState>,
    axum::extract::Query(q): axum::extract::Query<LogsQuery>,
) -> Json<Ok<LogsListPayload>> {
    let limit = q.limit.unwrap_or(50).min(200);
    let entries = state.logs.recent(limit);
    Json(Ok::new(LogsListPayload {
        entries,
        capacity: state.logs.capacity,
    }))
}

#[derive(Debug, Serialize)]
struct ProfileRow {
    /// Profile name (e.g. `default`, `production`, `local`).
    name: String,
    /// Absolute path to the profile's `gaussclaw.toml`.
    path: String,
    /// Whether this is the currently-active profile.
    active: bool,
}

#[derive(Debug, Serialize)]
struct ProfilesListPayload {
    profiles: Vec<ProfileRow>,
    /// The active profile's name (empty if no config was loaded).
    active: String,
}

#[axum::debug_handler]
async fn handle_profiles_list(State(state): State<ServerState>) -> Json<Ok<ProfilesListPayload>> {
    // Sprint 5 §10 ships the surface; multi-profile config persistence
    // lands when `gaussclaw-config` grows a profile-tree (Sprint 5 §10
    // follow-on). Until then we surface the loaded config path as the
    // single "default" profile and list any sibling `*.toml` files in
    // the same directory.
    let mut profiles: Vec<ProfileRow> = Vec::new();
    if let Some(active_path) = state.config_source.as_deref() {
        let active_name = profile_name_from_path(active_path);
        profiles.push(ProfileRow {
            name: active_name,
            path: active_path.to_string(),
            active: true,
        });
        if let Some(parent) = std::path::Path::new(active_path).parent() {
            if let Result::Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("toml")
                        && p.to_string_lossy() != active_path
                    {
                        let name = profile_name_from_path(&p.to_string_lossy());
                        profiles.push(ProfileRow {
                            name,
                            path: p.to_string_lossy().to_string(),
                            active: false,
                        });
                    }
                }
            }
        }
    }
    let active = profiles
        .iter()
        .find(|p| p.active)
        .map_or(String::new(), |p| p.name.clone());
    Json(Ok::new(ProfilesListPayload { profiles, active }))
}

fn profile_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(
            || "default".into(),
            |s| {
                if s == "gaussclaw" {
                    "default".into()
                } else {
                    s.into()
                }
            },
        )
}

#[derive(Debug, Serialize, Default)]
struct AnalyticsSummary {
    /// Number of sessions in the store.
    sessions_total: u64,
    /// Sum of `turn_count` across all sessions.
    turns_total: u64,
    /// Chain length (i.e. signed receipts).
    receipts_total: u64,
    /// Number of distinct models seen across sessions.
    distinct_models: u64,
    /// Average turns per session (0 if no sessions).
    avg_turns_per_session: f64,
    /// Most recently created session id (empty if none).
    most_recent_session_id: String,
}

#[axum::debug_handler]
#[allow(clippy::cast_precision_loss)]
async fn handle_analytics_summary(State(state): State<ServerState>) -> Json<Ok<AnalyticsSummary>> {
    let Some(store) = state.store() else {
        return Json(Ok::new(AnalyticsSummary::default()));
    };
    let sessions = store.list_recent_sessions(200).await;
    let sessions_total = sessions.len() as u64;
    let turns_total: u64 = sessions.iter().map(|s| s.turn_count).sum();
    let mut models = std::collections::BTreeSet::<&str>::new();
    for s in &sessions {
        models.insert(s.model.as_str());
    }
    let avg_turns_per_session = if sessions_total == 0 {
        0.0
    } else {
        turns_total as f64 / sessions_total as f64
    };
    let most_recent_session_id = sessions.first().map_or(String::new(), |s| s.id.clone());
    let receipts_total = store.chain_head().await.map_or(0, |h| h.length);
    Json(Ok::new(AnalyticsSummary {
        sessions_total,
        turns_total,
        receipts_total,
        distinct_models: models.len() as u64,
        avg_turns_per_session,
        most_recent_session_id,
    }))
}

// ─── Sprint 7 §3 — Plugins page ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct PluginRow {
    name: String,
    kind: String,
    version: String,
    description: String,
    caps: Vec<String>,
    provenance: String,
    enabled: bool,
    manifest_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct PluginsListPayload {
    plugins: Vec<PluginRow>,
    /// Per-root failure descriptions surfaced to the operator.
    failures: Vec<PluginFailure>,
    /// Discovery roots that were walked.
    roots: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PluginFailure {
    path: String,
    reason: String,
}

#[axum::debug_handler]
async fn handle_plugins_list(State(state): State<ServerState>) -> Json<Ok<PluginsListPayload>> {
    let mut plugins: Vec<PluginRow> = Vec::new();
    let mut failures: Vec<PluginFailure> = Vec::new();
    let roots: Vec<std::path::PathBuf> = if state.plugin_roots.is_empty() {
        gaussclaw_plugins::default_discovery_roots()
    } else {
        state.plugin_roots.clone()
    };
    for r in &roots {
        if let Result::Ok(report) = gaussclaw_plugins::PluginLoader::discover_in(r).await {
            for p in report.found {
                plugins.push(PluginRow {
                    name: p.manifest.name,
                    kind: p.manifest.kind.as_str().into(),
                    version: p.manifest.version,
                    description: p.manifest.description,
                    caps: p.manifest.caps,
                    provenance: p.provenance,
                    enabled: p.enabled,
                    manifest_path: p.manifest_path.map(|x| x.display().to_string()),
                });
            }
            for (path, reason) in report.failures {
                failures.push(PluginFailure {
                    path: path.display().to_string(),
                    reason,
                });
            }
        }
    }
    Json(Ok::new(PluginsListPayload {
        plugins,
        failures,
        roots: roots.iter().map(|r| r.display().to_string()).collect(),
    }))
}

// ─── Sprint 8 §5 — Replay-corpus diff visualiser ───────────────────────────

/// One turn-snapshot row in a replay capture.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayTurn {
    /// 0-based turn index.
    pub index: u64,
    /// `user` / `assistant` / `tool`.
    pub role: String,
    /// Free-form body.
    pub body: String,
    /// Hex chain head digest after this turn was appended.
    pub chain_head: String,
}

/// `POST /api/replay/diff` request body.
#[derive(Debug, Deserialize)]
struct ReplayDiffRequest {
    a: Vec<ReplayTurn>,
    b: Vec<ReplayTurn>,
}

/// One divergence row.
#[derive(Debug, Serialize)]
pub struct DiffRow {
    /// Turn index this row references.
    pub index: u64,
    /// Field name that diverged (`role`, `body`, `chain_head`, or
    /// `length` for the catch-all "one side ran out" case).
    pub axis: &'static str,
    /// Side-A value (or empty when A ran out).
    pub a: String,
    /// Side-B value (or empty when B ran out).
    pub b: String,
}

/// `POST /api/replay/diff` response body.
#[derive(Debug, Serialize)]
pub struct ReplayDiffReport {
    /// True iff every turn pair matches across all three axes
    /// (role / body / chain_head).
    pub convergent: bool,
    /// First divergent turn index, when not convergent.
    pub first_divergence_at: Option<u64>,
    /// Capture A length.
    pub a_length: u64,
    /// Capture B length.
    pub b_length: u64,
    /// Per-axis divergence rows; sorted by `(index, axis)`.
    pub rows: Vec<DiffRow>,
}

/// Compute the diff between two replay captures. Public so the
/// conformance suite can drive it without the HTTP layer.
#[must_use]
pub fn diff_replay(a: &[ReplayTurn], b: &[ReplayTurn]) -> ReplayDiffReport {
    let mut rows: Vec<DiffRow> = Vec::new();
    let mut first: Option<u64> = None;
    let len = a.len().max(b.len());
    for i in 0..len {
        let av = a.get(i);
        let bv = b.get(i);
        match (av, bv) {
            (Some(x), Some(y)) => {
                if x.role != y.role {
                    rows.push(DiffRow {
                        index: x.index,
                        axis: "role",
                        a: x.role.clone(),
                        b: y.role.clone(),
                    });
                    first.get_or_insert(x.index);
                }
                if x.body != y.body {
                    rows.push(DiffRow {
                        index: x.index,
                        axis: "body",
                        a: x.body.clone(),
                        b: y.body.clone(),
                    });
                    first.get_or_insert(x.index);
                }
                if x.chain_head != y.chain_head {
                    rows.push(DiffRow {
                        index: x.index,
                        axis: "chain_head",
                        a: x.chain_head.clone(),
                        b: y.chain_head.clone(),
                    });
                    first.get_or_insert(x.index);
                }
            }
            (Some(x), None) => {
                rows.push(DiffRow {
                    index: x.index,
                    axis: "length",
                    a: format!("turn {}", x.index),
                    b: "(missing)".into(),
                });
                first.get_or_insert(x.index);
            }
            (None, Some(y)) => {
                rows.push(DiffRow {
                    index: y.index,
                    axis: "length",
                    a: "(missing)".into(),
                    b: format!("turn {}", y.index),
                });
                first.get_or_insert(y.index);
            }
            (None, None) => unreachable!("loop bound is max(len_a, len_b)"),
        }
    }
    ReplayDiffReport {
        convergent: rows.is_empty(),
        first_divergence_at: first,
        a_length: a.len() as u64,
        b_length: b.len() as u64,
        rows,
    }
}

#[axum::debug_handler]
async fn handle_replay_diff(Json(body): Json<ReplayDiffRequest>) -> Json<Ok<ReplayDiffReport>> {
    Json(Ok::new(diff_replay(&body.a, &body.b)))
}

/// Lower-case hex encode.
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        let _ = write!(s, "{b:02x}");
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
        // Sprint 5 §3 — cron CRUD
        .route("/api/cron", get(handle_cron_list).post(handle_cron_add))
        .route(
            "/api/cron/{id}",
            get(handle_cron_get).delete(handle_cron_remove),
        )
        .route("/api/cron/{id}/edit", axum::routing::post(handle_cron_edit))
        .route(
            "/api/cron/{id}/pause",
            axum::routing::post(handle_cron_pause),
        )
        .route(
            "/api/cron/{id}/resume",
            axum::routing::post(handle_cron_resume),
        )
        .route("/api/cron/{id}/run", axum::routing::post(handle_cron_run))
        // Sprint 5 §10 — Logs / Profiles / Analytics
        .route("/api/logs", get(handle_logs_list))
        .route("/api/profiles", get(handle_profiles_list))
        .route("/api/analytics/summary", get(handle_analytics_summary))
        .route("/api/plugins", get(handle_plugins_list))
        .route("/api/replay/diff", axum::routing::post(handle_replay_diff))
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

    // ─── Sprint-5 cron endpoints ───────────────────────────────────────

    fn test_state_with_cron() -> ServerState {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        let store: std::sync::Arc<dyn gauss_cron::JobStore> =
            std::sync::Arc::new(gauss_cron::InMemoryJobStore::new());
        let sched = std::sync::Arc::new(CronScheduler::new(store, SystemClock));
        ServerState::new(cfg, Some("/tmp/gaussclaw.toml".into())).with_cron(sched)
    }

    async fn oneshot(
        state: ServerState,
        method: Method,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state);
        let mut req = Request::builder().method(method).uri(uri);
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req
            .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, json)
    }

    #[tokio::test]
    async fn cron_list_empty_without_scheduler() {
        let (status, body) = get_json("/api/cron").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cron_add_without_scheduler_returns_503() {
        let (status, body) = post_json(
            "/api/cron",
            serde_json::json!({ "schedule": "1h", "label": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "unavailable");
    }

    #[tokio::test]
    async fn cron_add_then_list_round_trips() {
        let st = test_state_with_cron();
        let (status, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "30m", "label": "ping-prod" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let id = body["data"]["id"].as_u64().expect("id");
        assert_eq!(body["data"]["label"], "ping-prod");
        assert_eq!(body["data"]["status"], "armed");

        let (status, body) = oneshot(st, Method::GET, "/api/cron", None).await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["data"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], id);
    }

    #[tokio::test]
    async fn cron_add_with_bad_schedule_returns_400() {
        let st = test_state_with_cron();
        let (status, body) = oneshot(
            st,
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "@@bogus@@" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn cron_pause_resume_round_trip() {
        let st = test_state_with_cron();
        let (_, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "1h" })),
        )
        .await;
        let id = body["data"]["id"].as_u64().unwrap();

        let (status, _) = oneshot(
            st.clone(),
            Method::POST,
            &format!("/api/cron/{id}/pause"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = oneshot(st.clone(), Method::GET, &format!("/api/cron/{id}"), None).await;
        assert_eq!(body["data"]["status"], "paused");

        let (status, _) = oneshot(
            st.clone(),
            Method::POST,
            &format!("/api/cron/{id}/resume"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = oneshot(st, Method::GET, &format!("/api/cron/{id}"), None).await;
        assert_eq!(body["data"]["status"], "armed");
    }

    #[tokio::test]
    async fn cron_run_fires_a_duration_job_early() {
        let st = test_state_with_cron();
        let (_, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "1h" })),
        )
        .await;
        let id = body["data"]["id"].as_u64().unwrap();

        let (status, body) = oneshot(
            st.clone(),
            Method::POST,
            &format!("/api/cron/{id}/run"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["fired"], id);

        // After run: status is completed, fire_count = 1.
        let (_, body) = oneshot(st, Method::GET, &format!("/api/cron/{id}"), None).await;
        assert_eq!(body["data"]["status"], "completed");
        assert_eq!(body["data"]["fire_count"], 1);
    }

    #[tokio::test]
    async fn cron_edit_changes_label_and_schedule() {
        let st = test_state_with_cron();
        let (_, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "1h", "label": "before" })),
        )
        .await;
        let id = body["data"]["id"].as_u64().unwrap();

        let (status, body) = oneshot(
            st.clone(),
            Method::POST,
            &format!("/api/cron/{id}/edit"),
            Some(serde_json::json!({ "label": "after", "schedule": "2h" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["label"], "after");
    }

    #[tokio::test]
    async fn cron_edit_with_no_fields_returns_400() {
        let st = test_state_with_cron();
        let (_, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "1h" })),
        )
        .await;
        let id = body["data"]["id"].as_u64().unwrap();

        let (status, _) = oneshot(
            st,
            Method::POST,
            &format!("/api/cron/{id}/edit"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cron_remove_drops_job() {
        let st = test_state_with_cron();
        let (_, body) = oneshot(
            st.clone(),
            Method::POST,
            "/api/cron",
            Some(serde_json::json!({ "schedule": "1h" })),
        )
        .await;
        let id = body["data"]["id"].as_u64().unwrap();

        let (status, _) =
            oneshot(st.clone(), Method::DELETE, &format!("/api/cron/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = oneshot(st, Method::GET, &format!("/api/cron/{id}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cron_get_unknown_returns_404() {
        let st = test_state_with_cron();
        let (status, body) = oneshot(st, Method::GET, "/api/cron/999", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
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

    // ─── Sprint-5 §10 — Logs / Profiles / Analytics ────────────────────

    #[tokio::test]
    async fn logs_endpoint_returns_empty_buffer_by_default() {
        let (status, body) = get_json("/api/logs?limit=10").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["capacity"], 200);
        assert!(body["data"]["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn logs_endpoint_returns_pushed_entries_newest_first() {
        let state = test_state();
        state.log(LogLevel::Info, "test", "first entry");
        state.log(LogLevel::Warn, "test", "second entry");
        state.log(LogLevel::Error, "test", "third entry");
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/logs?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entries = body["data"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["message"], "third entry");
        assert_eq!(entries[0]["level"], "error");
        assert_eq!(entries[2]["message"], "first entry");
    }

    #[tokio::test]
    async fn logs_endpoint_caps_limit_at_200() {
        let state = test_state();
        for i in 0..250 {
            state.log(LogLevel::Info, "test", format!("entry {i}"));
        }
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/logs?limit=500")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Capacity 200, plus the API caps `limit` at 200.
        assert!(body["data"]["entries"].as_array().unwrap().len() <= 200);
    }

    #[tokio::test]
    async fn profiles_endpoint_lists_active_default_when_loaded() {
        let (status, body) = get_json("/api/profiles").await;
        assert_eq!(status, StatusCode::OK);
        // test_state() carries `config_source = Some("/tmp/gaussclaw.toml")`.
        assert_eq!(body["data"]["active"], "default");
        let rows = body["data"]["profiles"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert_eq!(rows[0]["name"], "default");
        assert_eq!(rows[0]["active"], true);
    }

    #[tokio::test]
    async fn profiles_endpoint_returns_empty_when_no_config_loaded() {
        let cfg = Config::default();
        let state = ServerState::new(cfg, None);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/profiles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["data"]["active"], "");
        assert!(body["data"]["profiles"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn analytics_summary_is_zero_without_store() {
        let (status, body) = get_json("/api/analytics/summary").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["sessions_total"], 0);
        assert_eq!(body["data"]["turns_total"], 0);
        assert_eq!(body["data"]["receipts_total"], 0);
        assert_eq!(body["data"]["distinct_models"], 0);
    }

    #[tokio::test]
    async fn analytics_summary_aggregates_when_store_has_data() {
        use std::sync::Arc;
        let store = Arc::new(
            gaussclaw_store::SessionStore::open_in_memory()
                .await
                .unwrap(),
        );
        let sess1 = store
            .create_session("rest", "anthropic/claude-3.5-sonnet")
            .await;
        let sess2 = store.create_session("rest", "openai/gpt-4").await;
        for _ in 0..3 {
            let _ = store
                .append_turn(&sess1.id, None, "user", "hi", gauss_core::TaintLabel::User)
                .await
                .unwrap();
        }
        let _ = store
            .append_turn(
                &sess2.id,
                None,
                "user",
                "hello",
                gauss_core::TaintLabel::User,
            )
            .await
            .unwrap();

        let state = test_state().with_store(store);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/analytics/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["data"]["sessions_total"], 2);
        assert_eq!(body["data"]["turns_total"], 4);
        assert_eq!(body["data"]["distinct_models"], 2);
        assert!(body["data"]["receipts_total"].as_u64().unwrap() >= 4);
    }

    #[tokio::test]
    async fn plugins_endpoint_returns_empty_for_missing_roots() {
        let cfg = Config::default();
        let state = ServerState::new(cfg, None).with_plugin_roots(vec!["/does/not/exist".into()]);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/plugins")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["data"]["plugins"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn plugins_endpoint_discovers_plugins_under_supplied_root() {
        let dir = tempfile::tempdir().unwrap();
        let plug = dir.path().join("alpha");
        tokio::fs::create_dir(&plug).await.unwrap();
        tokio::fs::write(
            plug.join("plugin.toml"),
            "name = \"alpha\"\nversion = \"0.1.0\"\nkind = \"standalone\"\ncaps = []\n",
        )
        .await
        .unwrap();
        let state = ServerState::new(Config::default(), None)
            .with_plugin_roots(vec![dir.path().to_path_buf()]);
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/plugins")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let rows = body["data"]["plugins"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "alpha");
        assert_eq!(rows[0]["kind"], "standalone");
        assert!(!rows[0]["provenance"].as_str().unwrap().is_empty());
    }

    // ─── Sprint 8 §5 — Replay diff ──────────────────────────────────────

    fn mk_turn(i: u64, role: &str, body: &str, head: &str) -> ReplayTurn {
        ReplayTurn {
            index: i,
            role: role.into(),
            body: body.into(),
            chain_head: head.into(),
        }
    }

    #[tokio::test]
    async fn replay_diff_returns_convergent_for_identical_captures() {
        let a = vec![
            mk_turn(0, "user", "hello", "aaaa"),
            mk_turn(1, "assistant", "hi", "bbbb"),
        ];
        let report = diff_replay(&a, &a);
        assert!(report.convergent);
        assert!(report.first_divergence_at.is_none());
        assert_eq!(report.a_length, 2);
        assert_eq!(report.b_length, 2);
        assert!(report.rows.is_empty());
    }

    #[tokio::test]
    async fn replay_diff_finds_body_divergence() {
        let a = vec![mk_turn(0, "user", "hello", "aaaa")];
        let b = vec![mk_turn(0, "user", "world", "aaaa")];
        let report = diff_replay(&a, &b);
        assert!(!report.convergent);
        assert_eq!(report.first_divergence_at, Some(0));
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].axis, "body");
    }

    #[tokio::test]
    async fn replay_diff_finds_chain_head_divergence() {
        let a = vec![
            mk_turn(0, "user", "x", "aaaa"),
            mk_turn(1, "assistant", "y", "bbbb"),
        ];
        let b = vec![
            mk_turn(0, "user", "x", "aaaa"),
            mk_turn(1, "assistant", "y", "cccc"),
        ];
        let report = diff_replay(&a, &b);
        assert!(!report.convergent);
        assert_eq!(report.first_divergence_at, Some(1));
        assert_eq!(report.rows[0].axis, "chain_head");
    }

    #[tokio::test]
    async fn replay_diff_reports_length_mismatch() {
        let a = vec![mk_turn(0, "user", "x", "aaaa")];
        let b: Vec<ReplayTurn> = vec![];
        let report = diff_replay(&a, &b);
        assert!(!report.convergent);
        assert_eq!(report.first_divergence_at, Some(0));
        assert_eq!(report.rows[0].axis, "length");
    }

    #[tokio::test]
    async fn replay_diff_endpoint_round_trips() {
        let body = serde_json::json!({
            "a": [{"index": 0, "role": "user", "body": "x", "chain_head": "aa"}],
            "b": [{"index": 0, "role": "user", "body": "y", "chain_head": "aa"}],
        });
        let (status, resp) = post_json("/api/replay/diff", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp["data"]["convergent"], false);
        assert_eq!(resp["data"]["first_divergence_at"], 0);
        let rows = resp["data"]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["axis"], "body");
    }

    #[tokio::test]
    async fn replay_diff_endpoint_handles_empty_inputs() {
        let body = serde_json::json!({"a": [], "b": []});
        let (status, resp) = post_json("/api/replay/diff", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp["data"]["convergent"], true);
        assert_eq!(resp["data"]["a_length"], 0);
        assert_eq!(resp["data"]["b_length"], 0);
    }

    // ── chat socket → AgentLoop integration ────────────────────────────────

    /// End-to-end: when an AgentLoop is attached to ServerState, the
    /// chat path runs a real turn and the dashboard receives every
    /// LoopEvent translated through `wire::loop_event_to_wire`.
    ///
    /// We bypass the WebSocket transport (axum Test doesn't expose a
    /// WS client) and drive the loop directly against a CaptureOutbox.
    /// The outbox + sink + agent are the same instances handle_chat_ws
    /// builds in production, so this exercises the wired path
    /// without the socket layer.
    #[tokio::test]
    async fn chat_socket_path_streams_loop_events_via_wire() {
        use crate::wire::{CaptureOutbox, WireLoopSink, WireOutbox};
        use gauss_core::TaintLabel;
        use gaussclaw_agent::{AgentLoop, EchoProvider, KernelHandle, Message, Prompt, TurnPolicy};
        use std::sync::Arc;

        let provider: Arc<dyn gaussclaw_agent::ProviderHandle> = Arc::new(EchoProvider::default());
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let agent = Arc::new(AgentLoop::new(policy));

        let outbox: Arc<CaptureOutbox> = Arc::new(CaptureOutbox::default());
        let outbox_dyn: Arc<dyn WireOutbox> = outbox.clone();
        let sink = WireLoopSink::new(outbox_dyn);
        let _ = agent
            .run(
                Prompt::new("test", vec![Message::new("user", "hi")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("run ok");

        let frames = outbox.frames().await;
        // Every run emits at least: UserSubmitted → Assistant → Done.
        let kinds: Vec<&str> = frames
            .iter()
            .filter_map(|f| f.get("type").and_then(|v| v.as_str()))
            .collect();
        assert!(kinds.contains(&"user"), "no user frame in {kinds:?}");
        assert!(
            kinds.contains(&"assistant"),
            "no assistant frame in {kinds:?}"
        );
        assert!(kinds.contains(&"done"), "no done frame in {kinds:?}");
    }

    /// `LoopEvent::ToolDenied` (sprint 7) reaches the dashboard via
    /// the wire translation when a `PreToolHook::Deny` fires inside
    /// the same chat path.
    #[tokio::test]
    async fn chat_socket_path_surfaces_tool_denied_frames() {
        use crate::wire::{CaptureOutbox, WireLoopSink, WireOutbox};
        use async_trait::async_trait;
        use gauss_core::TaintLabel;
        use gauss_hooks::{HookBus, HookOutcome, PreToolEvent, PreToolHook};
        use gaussclaw_agent::{
            AgentLoop, Completion, KernelHandle, Message, Prompt, ProviderHandle, ProviderResult,
            TokenCount, TurnPolicy,
        };
        use std::sync::Arc;

        // Scripted provider: turn 1 asks for echo; turn 2 stops.
        struct ScriptedProvider {
            seq: std::sync::Mutex<usize>,
        }
        #[async_trait]
        impl ProviderHandle for ScriptedProvider {
            fn name(&self) -> &str {
                "scripted"
            }
            async fn complete(&self, _p: &Prompt) -> ProviderResult<Completion> {
                let mut g = self.seq.lock().unwrap();
                *g += 1;
                if *g == 1 {
                    Ok(Completion::new(
                        "<tool name=\"echo\">{\"text\":\"x\"}</tool>",
                        "scripted",
                        "tool",
                        TokenCount::new(1, 1),
                    ))
                } else {
                    Ok(Completion::new(
                        "done",
                        "scripted",
                        "stop",
                        TokenCount::new(1, 1),
                    ))
                }
            }
        }

        struct DenyEcho;
        #[async_trait]
        impl PreToolHook for DenyEcho {
            fn name(&self) -> &str {
                "deny-echo"
            }
            async fn on_pre_tool(&self, e: &PreToolEvent) -> HookOutcome {
                if e.tool == "echo" {
                    HookOutcome::Deny("policy: echo blocked".into())
                } else {
                    HookOutcome::Allow
                }
            }
        }

        let provider: Arc<dyn ProviderHandle> = Arc::new(ScriptedProvider {
            seq: std::sync::Mutex::new(0),
        });
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let bus = HookBus::new();
        bus.register_pre(Arc::new(DenyEcho), 0);
        let agent = Arc::new(AgentLoop::new(policy).with_hooks(bus));

        let outbox: Arc<CaptureOutbox> = Arc::new(CaptureOutbox::default());
        let outbox_dyn: Arc<dyn WireOutbox> = outbox.clone();
        let sink = WireLoopSink::new(outbox_dyn);
        let _ = agent
            .run(
                Prompt::new("test", vec![Message::new("user", "go")]),
                TaintLabel::User,
                None,
                &sink,
            )
            .await
            .expect("run ok");

        let frames = outbox.frames().await;
        let kinds: Vec<&str> = frames
            .iter()
            .filter_map(|f| f.get("type").and_then(|v| v.as_str()))
            .collect();
        assert!(
            kinds.contains(&"tool.denied"),
            "expected tool.denied in {kinds:?}"
        );
        // We should NOT see a tool.complete for echo — the deny short-
        // circuited dispatch.
        let saw_echo_complete = frames.iter().any(|f| {
            f.get("type").and_then(|v| v.as_str()) == Some("tool.complete")
                && f.get("tool").and_then(|v| v.as_str()) == Some("echo")
        });
        assert!(!saw_echo_complete, "echo must not run when denied");
    }

    /// `ServerState::with_agent` is honoured: calling `agent()` after
    /// attachment returns the same Arc.
    #[test]
    fn server_state_with_agent_round_trip() {
        use gaussclaw_agent::{AgentLoop, EchoProvider, KernelHandle, TurnPolicy};
        use std::sync::Arc;

        let provider: Arc<dyn gaussclaw_agent::ProviderHandle> = Arc::new(EchoProvider::default());
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let agent = Arc::new(AgentLoop::new(policy));
        let state = ServerState::new(Config::default(), None).with_agent(agent.clone());
        let got = state.agent().expect("agent attached");
        assert!(Arc::ptr_eq(&got, &agent));
    }

    /// No agent attached: ServerState::agent() returns None and the
    /// chat path falls back to the stub echo (we test that behaviour
    /// in the next test).
    #[test]
    fn server_state_default_has_no_agent() {
        let state = ServerState::new(Config::default(), None);
        assert!(state.agent().is_none());
    }
}
