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
        .record_inbound("gc_chat", "desktop", message.as_bytes(), TaintLabel::User, plane)
        .await;
    if let Err(e) = state.kernel().admit(CapToken::NETWORK_GET, TaintLabel::User) {
        return Envelope::err("denied", format!("{e:?}"));
    }
    // Phase 1 leaves the actual provider dispatch to `gaussclaw-surfaces`;
    // the desktop runtime forwards through the same `TurnPolicy` via the
    // shared `ServerState`. For the IPC-only test surface here we echo
    // back deterministically — the runtime wires the real path.
    Envelope::ok(format!("(desktop echo) {message}"))
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
        assert_ne!(before, after, "chat must advance audit head (WAL-before-effect)");
    }

    #[tokio::test]
    async fn chat_denied_when_kernel_lacks_caps() {
        use std::sync::Arc;
        use gauss_kernel::PrivilegedKernel;
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
