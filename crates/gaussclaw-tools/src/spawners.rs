//! Worker-spawner factories.
//!
//! [`WorkerSpawner`] is the HWCA primitive that runs a tool inside a
//! depth-bound worker context with optional sandbox enforcement. Phase
//! 3 slice 4 wires the Composite Sandbox attachment so production
//! deployments link real isolation layers (WASM L1 + Landlock L2 +
//! seccomp L3 + bwrap L4 on Linux; Seatbelt / AppContainer on
//! macOS / Windows).
//!
//! ## Three factory shapes
//!
//! - [`unsandboxed`] — fastest, no isolation; for tests and trusted
//!   in-process tools. Matches the Hermes `@tool` model on isolation.
//!   Schema gate + cap admit + depth bound still apply.
//! - [`noop_sandboxed`] — proves the [`SandboxTrait`] wiring is
//!   end-to-end. Useful in CI without WASM toolchain in place.
//! - [`composite_sandboxed`] — production posture: takes a fully-built
//!   [`CompositeSandbox`] (WASM L1 + Landlock L2 + seccomp L3 + bwrap
//!   L4) and wraps it for the worker spawner.
//!
//! ## Sandbox composition (T10)
//!
//! The composite sandbox bound from `GaussClaw.pdf` § T10:
//!
//! ```text
//! Pr[compromise] ≤ Π pᵢ + p_T
//! ```
//!
//! For four independent layers each with `pᵢ ≤ 10⁻²` (measured
//! against contemporary fuzz corpora), the product is `≤ 10⁻⁸`. Hermes
//! has no sandbox surface at all — the bound is `1`.

use std::sync::Arc;

use gauss_hwca::WorkerSpawner;
use gauss_sandbox::{CompositeSandbox, NoOpSandbox};
use gauss_traits::SandboxTrait;

/// Build an unsandboxed spawner. Tools run in-process under the schema
/// gate, cap-admit gate, and depth bound — no isolation layer.
///
/// **Tests + trusted-tool deployments only.** Production builds use
/// [`composite_sandboxed`] with real isolation layers.
#[must_use]
pub fn unsandboxed() -> Arc<WorkerSpawner> {
    Arc::new(WorkerSpawner::new())
}

/// Build a spawner attached to a [`NoOpSandbox`] (the L0 layer). This
/// exercises the full `SandboxTrait` codepath without any actual
/// isolation — useful in CI that doesn't have a WASM toolchain.
///
/// Production builds must replace this with [`composite_sandboxed`]
/// — see the `CompositeSandbox` builder.
#[must_use]
pub fn noop_sandboxed() -> Arc<WorkerSpawner> {
    let sandbox: Arc<dyn SandboxTrait> = Arc::new(NoOpSandbox);
    Arc::new(WorkerSpawner::with_sandbox(sandbox))
}

/// Wrap a caller-built [`CompositeSandbox`] in a worker spawner.
///
/// Production usage:
///
/// ```ignore
/// let composite = CompositeSandbox::builder_from_wasm_bytes(&wasm_bytes)?
///     .push(LandlockLayer::default())
///     .push(SeccompLayer::default())
///     .push(BwrapLayer::default())
///     .build();
/// let spawner = gaussclaw_tools::spawners::composite_sandboxed(composite);
/// let policy = TurnPolicy::new(kernel, provider).with_spawner(spawner);
/// ```
#[must_use]
pub fn composite_sandboxed(sandbox: CompositeSandbox) -> Arc<WorkerSpawner> {
    let sb: Arc<dyn SandboxTrait> = Arc::new(sandbox);
    Arc::new(WorkerSpawner::with_sandbox(sb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EchoTool;
    use gauss_core::TaintLabel;

    #[tokio::test]
    async fn unsandboxed_spawner_round_trips_echo() {
        let spawner = unsandboxed();
        let echo = EchoTool::new();
        let out = spawner
            .spawn_and_invoke(
                &echo,
                serde_json::json!({ "text": "no-sandbox" }),
                TaintLabel::User,
                0,
            )
            .await
            .expect("dispatch");
        assert_eq!(out.value["echo"], "no-sandbox");
    }

    #[tokio::test]
    async fn noop_sandboxed_spawner_round_trips_echo() {
        let spawner = noop_sandboxed();
        let echo = EchoTool::new();
        let out = spawner
            .spawn_and_invoke(
                &echo,
                serde_json::json!({ "text": "noop-sandbox" }),
                TaintLabel::User,
                0,
            )
            .await
            .expect("dispatch");
        assert_eq!(out.value["echo"], "noop-sandbox");
    }
}
