//! Shared application state held by the Tauri runtime.
//!
//! Holds a [`gaussclaw_web::ServerState`] so the IPC command surface
//! sees the same config, kernel, and audit trace as an `gaussclaw web`
//! HTTP server would — there is one source of truth, not two.

use gaussclaw_agent::{AuditTrace, KernelHandle};
use gaussclaw_config::Config;
use gaussclaw_web::ServerState;

/// Build the desktop app state.
///
/// Production deployments build the [`KernelHandle`] and [`AuditTrace`]
/// from the loaded config + the existing receipt chain; the Phase 1
/// helper [`new_default`] uses a permissive kernel and a fresh trace.
pub fn build(config: Config, kernel: KernelHandle, audit: AuditTrace) -> ServerState {
    ServerState::with_kernel_and_audit(config, None, kernel, audit)
}

/// Convenience: build a desktop state with a permissive kernel and a
/// fresh audit trace.
#[must_use]
pub fn new_default(config: Config) -> ServerState {
    build(config, KernelHandle::permissive(), AuditTrace::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_permissive_kernel() {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        let state = new_default(cfg);
        // The kernel handle is cheaply cloneable; just verify it's there.
        assert!(state.kernel().kernel().current_grant().bits() != 0);
    }
}
