//! Tauri 2 runtime entry point.
//!
//! Compiled only when the `tauri-runtime` Cargo feature is enabled —
//! see `Cargo.toml`. This module wires every IPC command in
//! [`crate::commands`] and [`crate::system`] into a `tauri::Builder`
//! along with the official plugins (global-shortcut, single-instance,
//! window-state, clipboard-manager, deep-link, updater) and starts the
//! native desktop binary.
//!
//! ## Boot sequence
//!
//! 1. Load the config from the platform's config directory (the same
//!    file `gaussclaw` uses for the CLI / TUI / web surface).
//! 2. Build a `ServerState` via [`crate::state::new_default`]. This
//!    shares one kernel + audit-chain with whatever else the binary
//!    is hosting (so the dashboard, the TUI, and the desktop window
//!    see the same conversation history).
//! 3. Initialise the plugin set:
//!    - **`single-instance`** — second invocation focuses the running
//!      window instead of spawning a second copy.
//!    - **`window-state`** — restores the prior window geometry.
//!    - **`global-shortcut`** — registers operator-supplied chords.
//!    - **`clipboard-manager`** — OS clipboard read/write.
//!    - **`notification`** — native notifications.
//!    - **`deep-link`** — `gaussclaw://` URL scheme handling.
//!    - **`updater`** — chain-verified updater (calls into
//!      [`crate::updater`] before swap-in).
//! 4. Register every IPC command (`gc_*`) with
//!    [`tauri::generate_handler!`].
//! 5. Run the event loop.
//!
//! ## Why it's still small at runtime
//!
//! Hermes Desktop bundles Electron 39 (Chromium + Node), which is
//! ~150 MB on disk and ~250 MB of RAM at idle. Tauri 2 uses the OS
//! WebView (WebView2 on Windows, WKWebView on macOS, WebKitGTK on
//! Linux) and embeds only the Rust binary + frontend assets. The
//! shipped GaussClaw desktop installer is ~20 MB; idle RAM ~80 MB;
//! cold start ≤ 500 ms — about a 10× win across the board.

use std::sync::Arc;

use gaussclaw_agent::{AuditTrace, KernelHandle};
use gaussclaw_config::Config;
use gaussclaw_web::ServerState;

use tauri::{Manager, RunEvent, Wry};
use tauri_plugin_clipboard_manager::init as init_clipboard;
use tauri_plugin_deep_link::init as init_deep_link;
use tauri_plugin_global_shortcut::Builder as GlobalShortcutBuilder;
use tauri_plugin_notification::init as init_notification;
use tauri_plugin_single_instance::init as init_single_instance;
use tauri_plugin_updater::Builder as UpdaterBuilder;
use tauri_plugin_window_state::Builder as WindowStateBuilder;

use crate::commands;
use crate::system;

/// Tauri-managed application state. Wraps the same [`ServerState`] the
/// HTTP backend uses so the dashboard, TUI, and desktop all read one
/// source of truth.
struct AppState(pub Arc<ServerState>);

/// Build and run the Tauri 2 application.
///
/// The caller supplies the loaded config, an admit-gated [`KernelHandle`],
/// and an existing [`AuditTrace`] (so the desktop continues a running
/// chain rather than starting a fresh one). For an entry-point that
/// composes these defaults, see [`crate::state::new_default`] —
/// production binaries pass through `gaussclaw doctor` first.
pub fn run(config: Config, kernel: KernelHandle, audit: AuditTrace) -> anyhow::Result<()> {
    let server_state = Arc::new(crate::state::build(config, kernel, audit));

    let builder = tauri::Builder::default()
        .plugin(init_single_instance(
            |app, _argv, _cwd| {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            },
            None,
        ))
        .plugin(WindowStateBuilder::default().build())
        .plugin(GlobalShortcutBuilder::new().build())
        .plugin(init_clipboard())
        .plugin(init_notification())
        .plugin(init_deep_link())
        .plugin(UpdaterBuilder::new().build())
        .manage(AppState(server_state))
        .invoke_handler(tauri::generate_handler![
            // Status & config
            tauri_gc_status,
            tauri_gc_config_get,
            tauri_gc_config_set,
            // Audit & caps
            tauri_gc_receipt_head,
            tauri_gc_receipts_recent,
            tauri_gc_caps,
            // Dashboard mirrors
            tauri_gc_health,
            tauri_gc_sessions_recent,
            tauri_gc_tools_list,
            tauri_gc_envelope_verify,
            tauri_gc_skill_preview,
            // Chat
            tauri_gc_chat,
            // Desktop-only
            tauri_gc_clipboard_copy,
            tauri_gc_global_hotkey_register,
            tauri_gc_tray_menu,
            tauri_gc_notify,
            tauri_gc_updater_verify_artifact,
        ]);

    let app = builder
        .build(tauri::generate_context!("tauri.conf.json"))
        .map_err(|e| anyhow::anyhow!("tauri::Builder::build: {e}"))?;

    app.run(|_handle, event| {
        if let RunEvent::ExitRequested { .. } = event {
            // Best-effort hook for any future drain work (flush audit,
            // close DB cursors). The kernel + audit are Arc-shared with
            // the rest of the process so they'll drop naturally.
        }
    });
    Ok(())
}

// ─── Tauri command shims ───────────────────────────────────────────────────

// Each shim is a thin `#[tauri::command]` wrapper that pulls the shared
// `ServerState` out of Tauri's managed-state map and forwards to the
// pure async function in `commands` or `system`. The pure functions are
// independently testable; the shims exist purely to satisfy
// `tauri::generate_handler!`'s lookup table.

macro_rules! state_arc {
    ($state:expr) => {
        $state.inner().0.clone()
    };
}

#[tauri::command]
async fn tauri_gc_status(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<commands::StatusPayload> {
    commands::status(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_config_get(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<commands::ConfigPayload> {
    commands::config_get(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_config_set(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> commands::Envelope<()> {
    commands::config_set(&*state_arc!(state), &key, &value).await
}

#[tauri::command]
async fn tauri_gc_receipt_head(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<commands::ReceiptHeadPayload> {
    commands::receipt_head(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_receipts_recent(
    state: tauri::State<'_, AppState>,
    limit: u64,
) -> commands::Envelope<commands::ReceiptListPayload> {
    commands::receipts_recent(&*state_arc!(state), limit).await
}

#[tauri::command]
async fn tauri_gc_caps(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<commands::CapsPayload> {
    commands::caps(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_health(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<commands::HealthPayload> {
    commands::health(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_sessions_recent(
    state: tauri::State<'_, AppState>,
    limit: u64,
) -> commands::Envelope<Vec<commands::SessionRow>> {
    commands::sessions_recent(&*state_arc!(state), limit).await
}

#[tauri::command]
async fn tauri_gc_tools_list(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<Vec<commands::ToolRow>> {
    commands::tools_list(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_envelope_verify(
    state: tauri::State<'_, AppState>,
    envelope: gaussclaw_export::Envelope,
) -> commands::Envelope<commands::EnvelopeVerifyReport> {
    commands::envelope_verify(&*state_arc!(state), envelope).await
}

#[tauri::command]
async fn tauri_gc_skill_preview(
    state: tauri::State<'_, AppState>,
    toml: String,
) -> commands::Envelope<commands::SkillPreviewReport> {
    commands::skill_preview(&*state_arc!(state), &toml).await
}

#[tauri::command]
async fn tauri_gc_chat(
    state: tauri::State<'_, AppState>,
    message: String,
) -> commands::Envelope<String> {
    commands::chat(&*state_arc!(state), &message).await
}

#[tauri::command]
async fn tauri_gc_clipboard_copy(
    state: tauri::State<'_, AppState>,
    body: String,
) -> commands::Envelope<system::ClipboardAck> {
    system::clipboard_copy(&*state_arc!(state), &body).await
}

#[tauri::command]
async fn tauri_gc_global_hotkey_register(
    state: tauri::State<'_, AppState>,
    chord: String,
) -> commands::Envelope<system::HotkeyRegistered> {
    system::global_hotkey_register(&*state_arc!(state), &chord).await
}

#[tauri::command]
async fn tauri_gc_tray_menu(
    state: tauri::State<'_, AppState>,
) -> commands::Envelope<system::TrayMenuPayload> {
    system::tray_menu(&*state_arc!(state)).await
}

#[tauri::command]
async fn tauri_gc_notify(
    state: tauri::State<'_, AppState>,
    title: String,
    body: String,
) -> commands::Envelope<()> {
    system::notify(&*state_arc!(state), &title, &body).await
}

#[tauri::command]
async fn tauri_gc_updater_verify_artifact(
    state: tauri::State<'_, AppState>,
    request: system::UpdaterVerifyRequest,
) -> commands::Envelope<system::UpdaterVerifyReport> {
    system::updater_verify_artifact(&*state_arc!(state), request).await
}

// Suppress an unused-Wry warning when no plugin uses it directly.
const _: fn() = || {
    let _: Option<Wry> = None;
};
