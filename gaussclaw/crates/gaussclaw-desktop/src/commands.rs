//! Tauri IPC command surface.
//!
//! Every public async function here corresponds to a Tauri `invoke`
//! channel the frontend can call. Functions are pure (no `tauri::`
//! types in their signatures) so they compile and test in any
//! environment; the [`runtime`] module wires them into a
//! `tauri::Builder` when the `tauri-runtime` feature is active.
//!
//! ## Naming
//!
//! Every command is prefixed `gc_` (for **g**auss**c**law). The
//! frontend calls them as e.g. `invoke('gc_status')`, mirroring the
//! upstream Hermes dashboard's `/api/status` HTTP path without the
//! HTTP framing.
//!
//! ## Capability gating
//!
//! Each command that mutates state goes through the kernel admit
//! gate (same `KernelHandle` the HTTP surface uses). This is the
//! structural win over Hermes Desktop: the *front-door* capability
//! discipline and the *tool-execution* capability discipline are
//! one artefact.

use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::SurfaceRequest;
use gaussclaw_config::Config;
use gaussclaw_export::{verify_envelope, Envelope as TrajectoryEnvelope, VerifyEnvelopeError};
use gaussclaw_skill::SkillManifest;
use gaussclaw_web::ServerState;
use serde::{Deserialize, Serialize};

use crate::build_info;

// ─── envelope ──────────────────────────────────────────────────────────────

/// IPC envelope. Mirrors the HTTP envelope in `gaussclaw-web` so the
/// frontend speaks the same shape over either transport.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "ok", content = "data", rename_all = "lowercase")]
pub enum Envelope<T> {
    /// Success.
    #[serde(rename = "true")]
    True(T),
    /// Failure (carries an error code + message).
    #[serde(rename = "false")]
    False {
        /// Stable machine-readable error id (`denied`, `bad_request`, …).
        code: String,
        /// Human-readable error message.
        message: String,
    },
}

impl<T> Envelope<T> {
    /// Wrap a payload in the success variant.
    pub const fn ok(payload: T) -> Self {
        Self::True(payload)
    }

    /// Build a failure envelope.
    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::False {
            code: code.into(),
            message: message.into(),
        }
    }
}

// ─── payloads ──────────────────────────────────────────────────────────────

/// `gc_status` payload.
#[derive(Debug, Serialize)]
pub struct StatusPayload {
    /// Crate version.
    pub version: String,
    /// `"debug"` / `"release"`.
    pub profile: String,
    /// True if the binary linked the Tauri 2 runtime (always true in
    /// the shipped binary).
    pub tauri_runtime: bool,
    /// Active provider name.
    pub provider: String,
    /// Active model id.
    pub model: String,
}

/// `gc_config_get` payload.
#[derive(Debug, Serialize)]
pub struct ConfigPayload {
    /// Source path the loader read (if any).
    pub source: Option<String>,
    /// Full config tree.
    pub config: Config,
}

/// `gc_receipt_head` payload.
#[derive(Debug, Serialize)]
pub struct ReceiptHeadPayload {
    /// Hex-encoded chain head digest.
    pub digest: String,
}

/// `gc_caps` payload — the live capability grant.
#[derive(Debug, Serialize)]
pub struct CapsPayload {
    /// Current grant bitmask.
    pub grant_bits: u64,
}

// ─── command implementations ───────────────────────────────────────────────

/// `gc_status` — version, profile, active provider / model.
pub async fn status(state: &ServerState) -> Envelope<StatusPayload> {
    Envelope::ok(StatusPayload {
        version: build_info::VERSION.into(),
        profile: build_info::PROFILE.into(),
        tauri_runtime: build_info::TAURI_RUNTIME,
        provider: state.config().provider.name.clone(),
        model: state.config().provider.model.clone(),
    })
}

/// `gc_config_get` — the active config tree.
pub async fn config_get(state: &ServerState) -> Envelope<ConfigPayload> {
    Envelope::ok(ConfigPayload {
        source: None,
        config: state.config().clone(),
    })
}

/// `gc_config_set` — capability-gated config write.
///
/// Production wiring sends this through the SAG approval plane (Phase
/// 7); Phase 1 returns the same 403-equivalent the HTTP backend does,
/// so dashboard authors see the contract.
pub async fn config_set(_state: &ServerState, _key: &str, _value: &str) -> Envelope<()> {
    Envelope::err(
        "denied",
        "config writes require the cap:config:write Skill Manifest (Phase 3)",
    )
}

/// `gc_receipt_head` — live audit-chain head.
pub async fn receipt_head(state: &ServerState) -> Envelope<ReceiptHeadPayload> {
    let head = state.audit().head().await;
    Envelope::ok(ReceiptHeadPayload {
        digest: head.to_hex(),
    })
}

/// `gc_caps` — the live capability grant. Useful for the desktop's
/// status bar (one of the Hermes-Ink-can't-show fields).
pub async fn caps(state: &ServerState) -> Envelope<CapsPayload> {
    let grant = state.kernel().kernel().current_grant();
    Envelope::ok(CapsPayload {
        grant_bits: grant.bits(),
    })
}

/// `gc_chat` — send a chat message through the agent loop. The IPC
/// version returns the unary response; streaming uses a separate
/// event channel.
pub async fn chat(state: &ServerState, message: &str) -> Envelope<String> {
    let plane = state.kernel().plane_for(SurfaceRequest::UserSync);
    // WAL-before-effect: audit-record before admit.
    state
        .audit()
        .record_inbound(
            "gc_chat",
            "desktop",
            message.as_bytes(),
            TaintLabel::User,
            plane,
        )
        .await;
    if let Err(e) = state
        .kernel()
        .admit(CapToken::NETWORK_GET, TaintLabel::User)
    {
        return Envelope::err("denied", format!("{e:?}"));
    }
    // Phase 1 leaves the actual provider dispatch to `gaussclaw-surfaces`;
    // the desktop runtime forwards through the same `TurnPolicy` via the
    // shared `ServerState`. For the IPC-only test surface here we echo
    // back deterministically — the runtime wires the real path.
    Envelope::ok(format!("(desktop echo) {message}"))
}

// ─── Sprint 3: dashboard-mirror commands ───────────────────────────────────

/// `gc_health` — the seven Self-Diagnostic Health Engine invariants.
///
/// Phase 1 returns the green-skeleton payload (mirrors `/api/health`);
/// Phase 2 wires this through `gauss-health` proper.
pub async fn health(_state: &ServerState) -> Envelope<HealthPayload> {
    Envelope::ok(HealthPayload {
        overall: "green".into(),
        invariants: vec![],
    })
}

/// `gc_health` payload.
#[derive(Debug, Serialize)]
pub struct HealthPayload {
    /// `"green"` / `"amber"` / `"red"`.
    pub overall: String,
    /// Per-invariant status rows.
    pub invariants: Vec<HealthInvariant>,
}

/// One Self-Diagnostic Health Engine invariant row.
#[derive(Debug, Serialize)]
pub struct HealthInvariant {
    /// Stable short name (`kernel`, `memory`, `audit`, …).
    pub name: String,
    /// `"ok"` / `"warn"` / `"err"`.
    pub status: String,
    /// Human-readable detail.
    pub detail: Option<String>,
}

/// `gc_sessions_recent` — recent sessions list. Mirrors the HTTP
/// `/api/sessions` endpoint; returns an empty list when no store is
/// attached so the dashboard's skeleton renders cleanly.
pub async fn sessions_recent(state: &ServerState, limit: u64) -> Envelope<Vec<SessionRow>> {
    let limit = limit.min(100) as usize;
    let Some(store) = state.store() else {
        return Envelope::ok(vec![]);
    };
    let rows = store
        .list_recent_sessions(limit)
        .await
        .into_iter()
        .map(|s| SessionRow {
            id: s.id,
            title: s.title,
            model: s.model,
            surface: s.surface,
            created: s.created,
            turn_count: s.turn_count,
        })
        .collect();
    Envelope::ok(rows)
}

/// One row in the sessions list.
#[derive(Debug, Serialize)]
pub struct SessionRow {
    /// Session id (short hex).
    pub id: String,
    /// Display title.
    pub title: String,
    /// Active model at the time the session was last touched.
    pub model: String,
    /// Surface that opened the session.
    pub surface: String,
    /// RFC 3339 creation timestamp.
    pub created: String,
    /// Number of turns observed.
    pub turn_count: u64,
}

/// `gc_receipts_recent` — recent receipts list with per-row verify status.
///
/// Mirrors `/api/receipts/recent`; bounded at limit ≤ 100.
pub async fn receipts_recent(state: &ServerState, limit: u64) -> Envelope<ReceiptListPayload> {
    let limit = limit.min(100);
    let Some(store) = state.store() else {
        return Envelope::ok(ReceiptListPayload {
            head: String::new(),
            length: 0,
            rows: vec![],
        });
    };
    let head = store.chain_head().await.ok();
    let length = head.as_ref().map_or(0, |h| h.length);
    let head_hex = head.map(|h| h.digest_hex).unwrap_or_default();
    let mut rows = Vec::with_capacity(limit as usize);
    if length > 0 {
        let start = length.saturating_sub(limit);
        for idx in (start..length).rev() {
            let turn_id = idx.saturating_add(1);
            let Some(receipt) = store.get_receipt(turn_id).await else {
                continue;
            };
            let verified = store.verify_receipt(turn_id).await.unwrap_or(false);
            rows.push(ReceiptRow {
                index: turn_id,
                digest: hex_lower(&receipt.post_head),
                payload_digest: hex_lower(&receipt.payload_digest),
                verified,
            });
        }
    }
    Envelope::ok(ReceiptListPayload {
        head: head_hex,
        length,
        rows,
    })
}

/// `gc_receipts_recent` payload.
#[derive(Debug, Serialize)]
pub struct ReceiptListPayload {
    /// Hex-encoded chain head.
    pub head: String,
    /// Chain length.
    pub length: u64,
    /// Recent receipts, most-recent first.
    pub rows: Vec<ReceiptRow>,
}

/// One row in the recent-receipts list.
#[derive(Debug, Serialize)]
pub struct ReceiptRow {
    /// 1-based chain index.
    pub index: u64,
    /// Hex-encoded post-head.
    pub digest: String,
    /// Hex-encoded payload digest.
    pub payload_digest: String,
    /// True when the receipt verifies under its embedded key.
    pub verified: bool,
}

/// `gc_envelope_verify` — verify a Cryptographic Trajectory Envelope.
///
/// Mirrors `POST /api/envelope/verify`; reports the failing axis on
/// failure (signature / payload_digest / chain_link / witness_head /
/// witness_index / public_key).
pub async fn envelope_verify(
    _state: &ServerState,
    envelope: TrajectoryEnvelope,
) -> Envelope<EnvelopeVerifyReport> {
    let chain_head_hex = hex_lower(&envelope.chain_head);
    let chain_length = envelope.chain_length;
    let has_anchor = envelope.tsa_anchor.is_some();
    match verify_envelope(&envelope, None, None) {
        Ok(()) => Envelope::ok(EnvelopeVerifyReport {
            verified: true,
            failed_axis: None,
            detail: None,
            chain_head: chain_head_hex,
            chain_length,
            has_anchor,
        }),
        Err(e) => Envelope::ok(EnvelopeVerifyReport {
            verified: false,
            failed_axis: Some(verify_axis(&e)),
            detail: Some(format!("{e}")),
            chain_head: chain_head_hex,
            chain_length,
            has_anchor,
        }),
    }
}

/// `gc_envelope_verify` payload.
#[derive(Debug, Serialize)]
pub struct EnvelopeVerifyReport {
    /// True iff every axis passed.
    pub verified: bool,
    /// Failing axis when verification failed.
    pub failed_axis: Option<&'static str>,
    /// Human-readable detail.
    pub detail: Option<String>,
    /// Hex-encoded chain head (echo).
    pub chain_head: String,
    /// Chain length (echo).
    pub chain_length: u64,
    /// True iff the envelope carried a TSA anchor (echo).
    pub has_anchor: bool,
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

/// `gc_skill_preview` — parse a Skill Manifest TOML and return a typed
/// summary (caps, taint, cost, IPI guard, max string length) without
/// installing it. Mirrors `POST /api/skills/preview`.
pub async fn skill_preview(_state: &ServerState, toml: &str) -> Envelope<SkillPreviewReport> {
    match SkillManifest::from_toml(toml) {
        Ok(m) => Envelope::ok(SkillPreviewReport {
            parsed: true,
            error: None,
            summary: Some(SkillSummary {
                name: m.name,
                description: m.description,
                usage: m.usage,
                caps: m.caps,
                taint: m.taint,
                reversible: m.reversible,
                persistent: m.persistent,
                cost_tokens_per_call: m.cost.tokens_per_call,
                cost_dollars_per_call: m.cost.dollars_per_call,
                no_instruction_substrings: m.guards.no_instruction_substrings,
                max_string_len: m.guards.max_string_len,
            }),
        }),
        Err(e) => Envelope::ok(SkillPreviewReport {
            parsed: false,
            error: Some(format!("{e}")),
            summary: None,
        }),
    }
}

/// `gc_skill_preview` payload.
#[derive(Debug, Serialize)]
pub struct SkillPreviewReport {
    /// True iff the TOML parsed cleanly.
    pub parsed: bool,
    /// Parse-error message when [`Self::parsed`] is false.
    pub error: Option<String>,
    /// Typed summary when [`Self::parsed`] is true.
    pub summary: Option<SkillSummary>,
}

/// Typed Skill Manifest summary surfaced through `gc_skill_preview`.
#[derive(Debug, Serialize)]
pub struct SkillSummary {
    /// Skill name.
    pub name: String,
    /// Description string.
    pub description: String,
    /// Usage string.
    pub usage: String,
    /// Capability requirement strings.
    pub caps: Vec<String>,
    /// Default output taint.
    pub taint: String,
    /// Whether the tool's external effect is reversible.
    pub reversible: bool,
    /// Whether the HWCA reuses a worker across calls.
    pub persistent: bool,
    /// Cost: tokens per call (manifest hint).
    pub cost_tokens_per_call: u32,
    /// Cost: dollars per call (manifest hint).
    pub cost_dollars_per_call: f64,
    /// True if the IPI substring guard is enabled.
    pub no_instruction_substrings: bool,
    /// Max string length the schema gate accepts.
    pub max_string_len: usize,
}

/// `gc_tools_list` — fall-back tool catalogue. Production deployments
/// populate this from the live `ToolRegistry`; the desktop scaffold
/// returns the canonical list so the dashboard renders cleanly even
/// before the registry is wired.
pub async fn tools_list(_state: &ServerState) -> Envelope<Vec<ToolRow>> {
    Envelope::ok(default_tool_catalogue())
}

/// One row in the tool catalogue.
#[derive(Debug, Serialize)]
pub struct ToolRow {
    /// Tool name (matches the Skill Manifest's `name`).
    pub name: String,
    /// Description string.
    pub description: String,
    /// Capability requirement label.
    pub cap: String,
    /// Default output taint.
    pub taint: String,
    /// Sandbox layers the tool runs inside.
    pub layers: Vec<String>,
}

fn default_tool_catalogue() -> Vec<ToolRow> {
    const ROWS: &[(&str, &str, &str, &str, &[&str])] = &[
        (
            "base64",
            "Encode and decode base64 strings.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "csv_parse",
            "RFC 4180 CSV → JSON.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "datetime",
            "Current time + RFC 3339 parsing.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        ("echo", "Reflect the input.", "cap:none", "⊥", &["WASM"]),
        (
            "env_get",
            "Read an allow-listed env var.",
            "cap:env:read",
            "user",
            &["WASM"],
        ),
        (
            "file_read",
            "Read a permitted file.",
            "cap:fs:read",
            "user",
            &["Landlock", "seccomp"],
        ),
        (
            "file_write",
            "Write to a permitted path.",
            "cap:fs:write",
            "user",
            &["Landlock", "seccomp"],
        ),
        (
            "hash",
            "SHA-256 / BLAKE3 digests.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "http_get",
            "HTTPS GET with header allowlist.",
            "cap:network:http_get",
            "web",
            &["WASM"],
        ),
        (
            "http_head",
            "HTTPS HEAD probe.",
            "cap:network:http_get",
            "web",
            &["WASM"],
        ),
        (
            "http_post",
            "HTTPS POST with body cap.",
            "cap:network:http_post",
            "web",
            &["WASM"],
        ),
        (
            "json_get",
            "RFC 6901 JSON Pointer get.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "json_set",
            "RFC 6901 JSON Pointer set.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "math_eval",
            "Pure-function arithmetic.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "regex_match",
            "Compiled regex matching.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
        (
            "shell",
            "Sandboxed shell command.",
            "cap:shell:exec",
            "web",
            &["WASM", "Landlock", "seccomp", "bwrap"],
        ),
        ("upper", "Uppercase a string.", "cap:none", "⊥", &["WASM"]),
        (
            "uuid",
            "Generate UUIDv4 / UUIDv7.",
            "cap:none",
            "⊥",
            &["WASM"],
        ),
    ];
    ROWS.iter()
        .map(|(n, d, c, t, l)| ToolRow {
            name: (*n).into(),
            description: (*d).into(),
            cap: (*c).into(),
            taint: (*t).into(),
            layers: l.iter().map(|s| (*s).into()).collect(),
        })
        .collect()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;

    fn test_state() -> ServerState {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        state::new_default(cfg)
    }

    fn unwrap_ok<T>(e: Envelope<T>) -> T {
        match e {
            Envelope::True(t) => t,
            Envelope::False { code, message } => {
                panic!("expected ok, got error code={code} message={message}")
            }
        }
    }

    #[tokio::test]
    async fn status_returns_active_provider_and_model() {
        let s = test_state();
        let r = unwrap_ok(status(&s).await);
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-3.5-sonnet");
        assert!(matches!(r.profile.as_str(), "debug" | "release"));
    }

    #[tokio::test]
    async fn config_get_returns_the_tree() {
        let s = test_state();
        let r = unwrap_ok(config_get(&s).await);
        assert_eq!(r.config.provider.name, "anthropic");
    }

    #[tokio::test]
    async fn config_set_is_denied_until_phase3() {
        let s = test_state();
        let r: Envelope<()> = config_set(&s, "provider.name", "openai").await;
        match r {
            Envelope::False { code, .. } => assert_eq!(code, "denied"),
            Envelope::True(()) => panic!("expected denied, got True"),
        }
    }

    #[tokio::test]
    async fn receipt_head_returns_hex_digest() {
        let s = test_state();
        let r = unwrap_ok(receipt_head(&s).await);
        assert_eq!(r.digest.len(), 64);
    }

    #[tokio::test]
    async fn caps_returns_a_nonzero_grant_under_permissive_kernel() {
        let s = test_state();
        let r = unwrap_ok(caps(&s).await);
        assert!(r.grant_bits > 0);
    }

    #[tokio::test]
    async fn chat_admit_passes_under_permissive_kernel() {
        let s = test_state();
        let r = unwrap_ok(chat(&s, "ping").await);
        assert!(r.contains("ping"));
    }

    #[tokio::test]
    async fn chat_advances_the_audit_head() {
        let s = test_state();
        let before = unwrap_ok(receipt_head(&s).await).digest;
        let _ = chat(&s, "ping").await;
        let after = unwrap_ok(receipt_head(&s).await).digest;
        assert_ne!(
            before, after,
            "chat must advance audit head (WAL-before-effect)"
        );
    }

    #[tokio::test]
    async fn health_returns_green_skeleton() {
        let s = test_state();
        let r = unwrap_ok(health(&s).await);
        assert_eq!(r.overall, "green");
        assert!(r.invariants.is_empty());
    }

    #[tokio::test]
    async fn sessions_recent_returns_empty_without_store() {
        let s = test_state();
        let r = unwrap_ok(sessions_recent(&s, 10).await);
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn receipts_recent_returns_empty_without_store() {
        let s = test_state();
        let r = unwrap_ok(receipts_recent(&s, 5).await);
        assert_eq!(r.length, 0);
        assert!(r.rows.is_empty());
    }

    #[tokio::test]
    async fn skill_preview_parses_minimal_toml() {
        let s = test_state();
        let r = unwrap_ok(
            skill_preview(
                &s,
                r#"
name = "echo"
description = "echo"
caps = []
taint = "trusted"
"#,
            )
            .await,
        );
        assert!(r.parsed);
        let summary = r.summary.expect("summary present");
        assert_eq!(summary.name, "echo");
        assert_eq!(summary.taint, "trusted");
    }

    #[tokio::test]
    async fn skill_preview_reports_invalid_toml() {
        let s = test_state();
        let r = unwrap_ok(skill_preview(&s, "this = is = not toml").await);
        assert!(!r.parsed);
        assert!(r.error.is_some());
    }

    #[tokio::test]
    async fn envelope_verify_axis_function_covers_every_variant() {
        // Smoke-test the verify_axis mapping. The full happy-path test
        // requires a real Ed25519-signed envelope; that integration test
        // lives in gaussclaw-export. Here we only verify the desktop
        // command's axis-string mapping is wired.
        use VerifyEnvelopeError::*;
        assert_eq!(verify_axis(&PublicKeyMismatch), "public_key");
        assert_eq!(verify_axis(&PayloadDigestMismatch), "payload_digest");
        assert_eq!(verify_axis(&ChainLinkInconsistent), "chain_link");
        assert_eq!(verify_axis(&Signature("fail".into())), "signature");
        assert_eq!(verify_axis(&WitnessHeadMismatch), "witness_head");
        assert_eq!(
            verify_axis(&WitnessIndexExceedsChain {
                index: 5,
                length: 2
            }),
            "witness_index"
        );
    }

    #[tokio::test]
    async fn tools_list_returns_full_catalogue() {
        let s = test_state();
        let r = unwrap_ok(tools_list(&s).await);
        assert_eq!(r.len(), 18);
        // Spot-check the HTTP family that Sprint 2 added.
        assert!(r.iter().any(|t| t.name == "http_get"));
        assert!(r.iter().any(|t| t.name == "http_post"));
        assert!(r.iter().any(|t| t.name == "http_head"));
    }

    #[tokio::test]
    async fn chat_denied_when_kernel_lacks_caps() {
        use gauss_kernel::PrivilegedKernel;
        use std::sync::Arc;
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        let bottom = Arc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let s = state::build(
            cfg,
            gaussclaw_agent::KernelHandle::new(bottom),
            gaussclaw_agent::AuditTrace::new(),
        );
        let r: Envelope<String> = chat(&s, "ping").await;
        match r {
            Envelope::False { code, .. } => assert_eq!(code, "denied"),
            Envelope::True(t) => panic!("expected denied, got {t}"),
        }
    }
}
