//! `gaussclaw-agent` — agent-loop glue + kernel handle for the surfaces.
//!
//! Phase 1 Tasks 9 + 10 of `GAUSSCLAW_ROADMAP.md`. Two responsibilities,
//! delivered incrementally:
//!
//! 1. **Kernel gate (this slice).** A shared [`KernelHandle`] that every
//!    surface (`gaussclaw-web`, `gaussclaw-surfaces`, channel adapters)
//!    consults before processing a request. The handle wraps an
//!    `Arc<dyn Kernel>` and exposes [`KernelHandle::admit`] +
//!    [`KernelHandle::plane_for`] so all surfaces share one
//!    capability/taint gate.
//!
//! 2. **Turn policy (later slice).** The Hermes `run_conversation` body
//!    lifted into a `Differential Turn Engine` policy — assembles a
//!    prompt, dispatches to the provider plane, parses tool calls,
//!    repeats until done, writes the turn record. Stubbed today; lands
//!    once `gaussclaw-providers` ships in Phase 4.

#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use gauss_core::{CapToken, GaussResult, TaintLabel};
use gauss_kernel::{Plane, PrivilegedKernel};
use gauss_traits::Kernel;

// ─── kernel handle ──────────────────────────────────────────────────────────

/// Shared handle every surface holds in its state.
///
/// Carries the [`Kernel`] implementation (so admit gates work) and a
/// [`PlaneSelector`] policy (so every surface routes through the right
/// scheduler plane). Cheap to `Clone` — internally `Arc<dyn Kernel>`.
#[derive(Clone)]
pub struct KernelHandle {
    inner: Arc<dyn Kernel>,
    selector: PlaneSelector,
}

impl KernelHandle {
    /// Wrap an existing kernel implementation.
    pub fn new(kernel: Arc<dyn Kernel>) -> Self {
        Self {
            inner: kernel,
            selector: PlaneSelector::default(),
        }
    }

    /// Convenience: build a permissive privileged kernel (`CapToken::TOP`)
    /// with the default declassification map. Use this for tests and for
    /// the Phase 1 demo binary, where no real grant pipeline exists yet.
    #[must_use]
    pub fn permissive() -> Self {
        Self::new(Arc::new(PrivilegedKernel::new(CapToken::TOP)))
    }

    /// Borrow the inner kernel — useful for code that wants to call
    /// [`Kernel::current_grant`] directly.
    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        &self.inner
    }

    /// Joint capability/taint admission check. Forwards to the underlying
    /// [`Kernel::admit`]; returns `GaussError::Denied` /
    /// `GaussError::TaintTooHigh` on failure.
    pub fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()> {
        self.inner.admit(required, taint)
    }

    /// Map a surface-side request descriptor to the scheduler plane that
    /// owns its budget pool. The mapping is data — replace
    /// [`PlaneSelector`] on the handle to customise per deployment.
    #[must_use]
    pub const fn plane_for(&self, req: SurfaceRequest) -> Plane {
        self.selector.plane_for(req)
    }

    /// Swap the [`PlaneSelector`] policy.
    #[must_use]
    pub const fn with_selector(mut self, selector: PlaneSelector) -> Self {
        self.selector = selector;
        self
    }
}

impl std::fmt::Debug for KernelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KernelHandle")
            .field("grant", &self.inner.current_grant())
            .field("selector", &self.selector)
            .finish()
    }
}

// ─── plane selector ─────────────────────────────────────────────────────────

/// Surface-side request descriptor — what the kernel uses to pick a plane.
///
/// Every variant has a canonical mapping to one of the three scheduler
/// planes (Conversation / Daemon / Approval). The mapping is structural:
/// user-synchronous traffic goes to Conversation, background turns go to
/// Daemon, human-in-the-loop round trips go to Approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceRequest {
    /// CLI / TUI / REST / WS / OAI-compat — all user-synchronous.
    UserSync,
    /// `/v1/chat/completions` and `/v1/completions` — SDK chat traffic.
    SdkChat,
    /// Messaging-gateway ingress (Slack, Discord, Telegram, …).
    Channel,
    /// Scheduled or daemon-launched turn (cron, background sweeps).
    Scheduled,
    /// Human-in-the-loop approval prompt.
    Approval,
}

/// Policy that maps a [`SurfaceRequest`] to a scheduler [`Plane`].
///
/// The default policy follows the roadmap exactly:
///
/// | request                | plane          |
/// |---|---|
/// | `UserSync`, `SdkChat`, `Channel` | `Conversation` |
/// | `Scheduled`            | `Daemon`       |
/// | `Approval`             | `Approval`     |
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlaneSelector;

impl Default for PlaneSelector {
    fn default() -> Self {
        Self
    }
}

impl PlaneSelector {
    /// Return the plane for a given request descriptor.
    #[must_use]
    pub const fn plane_for(&self, req: SurfaceRequest) -> Plane {
        match req {
            SurfaceRequest::UserSync | SurfaceRequest::SdkChat | SurfaceRequest::Channel => {
                Plane::Conversation
            }
            SurfaceRequest::Scheduled => Plane::Daemon,
            SurfaceRequest::Approval => Plane::Approval,
        }
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{CapToken, TaintLabel};

    #[test]
    fn permissive_kernel_admits_a_trusted_request() {
        let h = KernelHandle::permissive();
        h.admit(CapToken::FILESYSTEM_READ, TaintLabel::Trusted)
            .expect("permissive kernel should admit a trusted read");
    }

    #[test]
    fn permissive_kernel_denies_post_under_web_taint() {
        let h = KernelHandle::permissive();
        let err = h
            .admit(CapToken::NETWORK_POST, TaintLabel::Web)
            .expect_err("default declass blocks POST under Web taint");
        // Underlying error is a `Denied` / `TaintTooHigh`; we don't care
        // which exact variant for this gate test — just that it errors.
        let _ = err;
    }

    #[test]
    fn plane_selector_maps_user_sync_to_conversation() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::UserSync), Plane::Conversation);
        assert_eq!(h.plane_for(SurfaceRequest::SdkChat), Plane::Conversation);
        assert_eq!(h.plane_for(SurfaceRequest::Channel), Plane::Conversation);
    }

    #[test]
    fn plane_selector_maps_scheduled_to_daemon() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::Scheduled), Plane::Daemon);
    }

    #[test]
    fn plane_selector_maps_approval_to_approval() {
        let h = KernelHandle::permissive();
        assert_eq!(h.plane_for(SurfaceRequest::Approval), Plane::Approval);
    }
}
