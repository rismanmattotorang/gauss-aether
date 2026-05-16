//! `gauss-sandbox` — composite execution sandbox (Phase 3).
//!
//! Implements [`gauss_traits::SandboxTrait`] over a stack of orthogonal
//! confinement layers (paper §IX, Theorem T10):
//!
//! ```text
//!  ┌─────────────────────────────────────────────────────────────┐
//!  │ L1  WASM (wasmi; fuel + step-budget interruption)           │
//!  │ L2  Linux Landlock 5.13+ / macOS Seatbelt                   │
//!  │ L3a Linux user namespaces (via bubblewrap subprocess)       │
//!  │ L3b Linux seccomp filter                                    │
//!  │ L4  TEE attestation (Phase 10)                              │
//!  └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! The Phase-3 [`CompositeSandbox`] composes the available layers per
//! capability (see [`gauss_traits::min_sandbox_for`]). When a layer is not
//! available on the build target the executor records the gap on the
//! outcome's `layers_invoked` so the conformance suite can verify the
//! product bound under the right hypotheses.

#![allow(clippy::module_name_repetitions)]

pub mod composite;
pub mod noop;
pub mod wasm;

#[cfg(all(target_os = "linux", feature = "linux-layers"))]
pub mod landlock_layer;
#[cfg(all(target_os = "linux", feature = "linux-layers"))]
pub mod seccomp_layer;

#[cfg(target_os = "linux")]
pub mod bwrap_layer;
#[cfg(target_os = "macos")]
pub mod seatbelt_layer;

pub use composite::{CompositeSandbox, CompositeSandboxBuilder};
pub use noop::NoOpSandbox;
#[cfg(feature = "wasm-wasmi")]
pub use wasm::WasmSandbox;
