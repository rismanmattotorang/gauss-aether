//! `gauss-exec` — session executor abstraction.
//!
//! Sprint 6 §1 of `/ROADMAP.md`. Hermes ships per-backend
//! environment classes under `tools/environments/` (local, docker,
//! ssh, modal, singularity, daytona, vercel sandbox). Each runs the
//! current session under raw operator credentials with no in-process
//! containment.
//!
//! `gauss-exec`'s shipping surface:
//!
//! - **[`SessionExecutor`]** trait — a single async `exec(req)` method
//!   that dispatches a command + args + env + working-directory into
//!   the executor's runtime and streams back an [`ExecOutput`].
//! - **[`LocalExecutor`]** — reference impl that runs commands in the
//!   current process via `tokio::process::Command`. Matches the
//!   pre-Sprint-6 behaviour but threads through the kernel admit
//!   gate.
//! - **[`ExecRouter`]** — dispatches a request to the right executor
//!   based on a `Backend` tag. Re-checks the corresponding
//!   `cap:executor:<backend>` cap on every call — defence in depth
//!   above the kernel.
//!
//! ## Hermes-superiority axes
//!
//! - **Per-backend cap separation.** Hermes runs every backend under
//!   the same `subprocess:spawn` credential; we mint distinct caps
//!   (`executor:local`, `executor:docker`, `executor:ssh`,
//!   `executor:modal`) so an operator can grant local-only execution.
//! - **Deterministic outputs.** `ExecOutput` is `serde`-tagged and
//!   stable; the conformance suite drives it with a stub `MockExecutor`.
//! - **Audit-aware.** Every exec returns a [`Receipt`] the caller
//!   appends to the chain. Hermes ships no audit linkage.
//!
//! The shipping crate includes `LocalExecutor` only; Docker / SSH /
//! Modal land as follow-on commits (each adds a new module + a new
//! cap that's already reserved in `gauss-core::CapToken`).

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_many_lines
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod local;
pub mod router;
pub mod types;

pub use local::LocalExecutor;
pub use router::{ExecRouter, ExecRouterError};
pub use types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};
