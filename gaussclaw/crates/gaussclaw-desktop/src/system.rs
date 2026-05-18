//! Desktop-only IPC commands.
//!
//! These commands surface platform-native capabilities that the HTTP
//! dashboard cannot offer: global hotkeys, system tray menus, OS
//! clipboard access, native notifications, and the chain-verified
//! updater.
//!
//! Every public async function is **pure** — no `tauri::` types in
//! signatures — so the command surface is testable without the Tauri
//! runtime. The [`crate::runtime`] module wires each function into a
//! `tauri::Builder` only when the `tauri-runtime` feature is on, so
//! the live integration is feature-gated but the contract is always
//! linkable.

use gauss_core::{CapToken, TaintLabel};
use gaussclaw_agent::SurfaceRequest;
use gaussclaw_web::ServerState;
use serde::{Deserialize, Serialize};

use crate::commands::Envelope;
use crate::updater::{verify_release_artifact, ReleaseManifest};

// ─── clipboard ─────────────────────────────────────────────────────────────

/// `gc_clipboard_copy` — copy a string to the system clipboard.
///
/// Audit-recorded before the OS clipboard is touched so the WAL barrier
/// (Axiom A1) holds for an externally visible side-effect. The runtime
/// layer plugs in [`tauri-plugin-clipboard-manager`] to do the actual
/// write; the pure function here returns the audit ack.
pub async fn clipboard_copy(state: &ServerState, body: &str) -> Envelope<ClipboardAck> {
    let plane = state.kernel().plane_for(SurfaceRequest::UserSync);
    state
        .audit()
        .record_inbound(
            "gc_clipboard_copy",
            "desktop",
            body.as_bytes(),
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
    Envelope::ok(ClipboardAck {
        copied_bytes: body.len(),
    })
}

/// `gc_clipboard_copy` payload.
#[derive(Debug, Serialize)]
pub struct ClipboardAck {
    /// Number of bytes the OS clipboard plug-in was asked to write.
    pub copied_bytes: usize,
}

// ─── global hotkey ─────────────────────────────────────────────────────────

/// `gc_global_hotkey_register` — register a global keyboard shortcut.
///
/// Accepts the standard chord notation (`"CommandOrControl+Shift+G"`).
/// The kernel admit gate enforces a sane upper-bound on registered
/// hotkeys before the OS plug-in actually touches the global table.
pub async fn global_hotkey_register(
    state: &ServerState,
    chord: &str,
) -> Envelope<HotkeyRegistered> {
    if !is_well_formed_chord(chord) {
        return Envelope::err(
            "bad_request",
            format!("`{chord}` is not a well-formed hotkey chord"),
        );
    }
    let plane = state.kernel().plane_for(SurfaceRequest::UserSync);
    state
        .audit()
        .record_inbound(
            "gc_global_hotkey_register",
            "desktop",
            chord.as_bytes(),
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
    Envelope::ok(HotkeyRegistered {
        chord: chord.into(),
    })
}

/// `gc_global_hotkey_register` payload.
#[derive(Debug, Serialize)]
pub struct HotkeyRegistered {
    /// The chord the plugin will surface to the OS shortcut table.
    pub chord: String,
}

/// Loose grammar check for hotkey chords. The Tauri plugin uses the
/// `accelerator` parser; we mirror its accepted modifier set here.
fn is_well_formed_chord(chord: &str) -> bool {
    if chord.is_empty() {
        return false;
    }
    const MODIFIERS: &[&str] = &[
        "command",
        "cmd",
        "control",
        "ctrl",
        "commandorcontrol",
        "cmdorctrl",
        "alt",
        "option",
        "shift",
        "super",
        "meta",
    ];
    let parts: Vec<&str> = chord.split('+').collect();
    if parts.len() < 2 {
        return false;
    }
    let key = parts.last().unwrap_or(&"").to_ascii_lowercase();
    if key.is_empty() {
        return false;
    }
    parts
        .iter()
        .take(parts.len().saturating_sub(1))
        .all(|m| MODIFIERS.contains(&m.to_ascii_lowercase().as_str()))
}

// ─── tray ──────────────────────────────────────────────────────────────────

/// `gc_tray_menu` — return the operator-configurable tray menu.
///
/// The runtime layer renders this through `tauri::tray`; the pure
/// function exposes the model so the React frontend can preview the
/// menu layout in Settings before the user commits.
pub async fn tray_menu(_state: &ServerState) -> Envelope<TrayMenuPayload> {
    Envelope::ok(TrayMenuPayload {
        items: vec![
            TrayMenuItem {
                id: "show".into(),
                label: "Show GaussClaw".into(),
                accel: Some("CommandOrControl+Shift+G".into()),
                kind: "action".into(),
            },
            TrayMenuItem {
                id: "doctor".into(),
                label: "Run doctor".into(),
                accel: None,
                kind: "action".into(),
            },
            TrayMenuItem {
                id: "separator-1".into(),
                label: String::new(),
                accel: None,
                kind: "separator".into(),
            },
            TrayMenuItem {
                id: "quit".into(),
                label: "Quit".into(),
                accel: Some("CommandOrControl+Q".into()),
                kind: "action".into(),
            },
        ],
    })
}

/// `gc_tray_menu` payload.
#[derive(Debug, Serialize)]
pub struct TrayMenuPayload {
    /// Ordered tray-menu items.
    pub items: Vec<TrayMenuItem>,
}

/// One tray-menu row.
#[derive(Debug, Serialize)]
pub struct TrayMenuItem {
    /// Stable id the runtime dispatcher routes on.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional accelerator string (e.g. `"CommandOrControl+Shift+G"`).
    pub accel: Option<String>,
    /// `"action"` or `"separator"`.
    pub kind: String,
}

// ─── notifications ─────────────────────────────────────────────────────────

/// `gc_notify` — surface a native OS notification.
pub async fn notify(state: &ServerState, title: &str, body: &str) -> Envelope<()> {
    let plane = state.kernel().plane_for(SurfaceRequest::Scheduled);
    state
        .audit()
        .record_inbound(
            "gc_notify",
            "desktop",
            format!("{title}: {body}").as_bytes(),
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
    Envelope::ok(())
}

// ─── updater ───────────────────────────────────────────────────────────────

/// `gc_updater_verify_artifact` — independently verify a downloaded
/// release artefact before Tauri's updater swaps it in.
///
/// Hermes ships unsigned binaries with no chain anchor; its updater
/// verifies nothing. GaussClaw verifies:
///
/// 1. The artefact's SHA-256 binds to the publisher manifest.
/// 2. The publisher's Ed25519 signature over `version:target:sha256`.
/// 3. The artefact's target triple matches the host.
/// 4. The version is strictly newer than the running one.
pub async fn updater_verify_artifact(
    state: &ServerState,
    request: UpdaterVerifyRequest,
) -> Envelope<UpdaterVerifyReport> {
    let plane = state.kernel().plane_for(SurfaceRequest::Scheduled);
    state
        .audit()
        .record_inbound(
            "gc_updater_verify_artifact",
            "desktop",
            request.manifest.sha256_hex.as_bytes(),
            TaintLabel::User,
            plane,
        )
        .await;
    // Decode artefact bytes (base64-armoured on the wire).
    let bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &request.artefact_base64,
    ) {
        Ok(b) => b,
        Err(e) => return Envelope::err("bad_request", format!("base64: {e}")),
    };
    // Decode publisher key.
    let pk_bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &request.publisher_pk_base64,
    ) {
        Ok(b) => b,
        Err(e) => return Envelope::err("bad_request", format!("publisher key base64: {e}")),
    };
    let Ok(arr): Result<[u8; 32], _> = pk_bytes.as_slice().try_into() else {
        return Envelope::err("bad_request", "publisher key must be 32 bytes");
    };
    let pk = match ed25519_dalek::VerifyingKey::from_bytes(&arr) {
        Ok(k) => k,
        Err(e) => return Envelope::err("bad_request", format!("publisher key: {e}")),
    };
    match verify_release_artifact(
        &request.manifest,
        &bytes,
        &pk,
        &request.running_version,
        &request.host_target,
    ) {
        Ok(()) => Envelope::ok(UpdaterVerifyReport {
            verified: true,
            failed_axis: None,
            detail: None,
        }),
        Err(e) => Envelope::ok(UpdaterVerifyReport {
            verified: false,
            failed_axis: Some(axis_of(&e)),
            detail: Some(format!("{e}")),
        }),
    }
}

/// `gc_updater_verify_artifact` request.
#[derive(Debug, Deserialize)]
pub struct UpdaterVerifyRequest {
    /// Release manifest from the publisher.
    pub manifest: ReleaseManifest,
    /// Downloaded artefact bytes, base64-encoded.
    pub artefact_base64: String,
    /// Publisher's Ed25519 public key, 32 raw bytes, base64-encoded.
    pub publisher_pk_base64: String,
    /// SemVer string of the currently running binary.
    pub running_version: String,
    /// Rust-style target triple of the running host.
    pub host_target: String,
}

/// `gc_updater_verify_artifact` response.
#[derive(Debug, Serialize)]
pub struct UpdaterVerifyReport {
    /// True iff every verification axis passed.
    pub verified: bool,
    /// Stable identifier of the failing axis when [`Self::verified`] is false.
    pub failed_axis: Option<&'static str>,
    /// Human-readable detail.
    pub detail: Option<String>,
}

const fn axis_of(e: &crate::updater::UpdaterVerifyError) -> &'static str {
    use crate::updater::UpdaterVerifyError::*;
    match e {
        Sha256Mismatch { .. } => "sha256",
        BadEncoding(_) => "encoding",
        BadPublisherSignature => "publisher_signature",
        TargetMismatch { .. } => "target",
        VersionNotGreater { .. } => "version",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Envelope;
    use crate::state;
    use gauss_core::CapToken;
    use gaussclaw_config::Config;

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
                panic!("expected ok, got code={code} message={message}")
            }
        }
    }

    #[tokio::test]
    async fn clipboard_copy_records_audit_and_returns_byte_count() {
        let s = test_state();
        let r = unwrap_ok(clipboard_copy(&s, "hello").await);
        assert_eq!(r.copied_bytes, 5);
    }

    #[tokio::test]
    async fn global_hotkey_accepts_well_formed_chord() {
        let s = test_state();
        let r = unwrap_ok(global_hotkey_register(&s, "CommandOrControl+Shift+G").await);
        assert_eq!(r.chord, "CommandOrControl+Shift+G");
    }

    #[tokio::test]
    async fn global_hotkey_refuses_malformed_chord() {
        let s = test_state();
        let r = global_hotkey_register(&s, "blarg+Q").await;
        match r {
            Envelope::False { code, .. } => assert_eq!(code, "bad_request"),
            Envelope::True(_) => panic!("expected bad_request"),
        }
    }

    #[tokio::test]
    async fn global_hotkey_refuses_bare_key() {
        let s = test_state();
        match global_hotkey_register(&s, "Q").await {
            Envelope::False { code, .. } => assert_eq!(code, "bad_request"),
            Envelope::True(_) => panic!("expected bad_request"),
        }
    }

    #[tokio::test]
    async fn tray_menu_contains_quit() {
        let s = test_state();
        let r = unwrap_ok(tray_menu(&s).await);
        assert!(r.items.iter().any(|i| i.id == "quit"));
        // Separator items appear with empty labels.
        assert!(r.items.iter().any(|i| i.kind == "separator"));
    }

    #[tokio::test]
    async fn notify_succeeds_under_permissive_kernel() {
        let s = test_state();
        match notify(&s, "title", "body").await {
            Envelope::True(()) => {}
            Envelope::False { code, message } => {
                panic!("expected ok, got code={code} message={message}")
            }
        }
    }

    #[tokio::test]
    async fn updater_verify_returns_axis_on_failure() {
        let s = test_state();
        let req = UpdaterVerifyRequest {
            manifest: ReleaseManifest {
                version: "0.0.0".into(),
                target: "x86_64-unknown-linux-gnu".into(),
                sha256_hex: "00".repeat(32),
                publisher_signature_hex: "00".repeat(64),
                chain_index: 0,
            },
            artefact_base64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                b"x",
            ),
            publisher_pk_base64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &[0u8; 32],
            ),
            running_version: "1.0.0".into(),
            host_target: "x86_64-unknown-linux-gnu".into(),
        };
        let r = unwrap_ok(updater_verify_artifact(&s, req).await);
        assert!(!r.verified);
        // 0.0.0 < 1.0.0 ⇒ downgrade refused first, so failed_axis = "version".
        assert_eq!(r.failed_axis, Some("version"));
    }
}
