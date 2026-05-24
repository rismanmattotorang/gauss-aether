//! `gauss-sop` — Standard Operating Procedure engine.
//!
//! Sprint 14 §1 of `/docs/SPRINT_ZEROCLAW_SOP.md`. `ZeroClaw` ships
//! an SOP subsystem that glues event triggers (MQTT / webhooks /
//! cron / peripherals) to approval-gated workflows with resumable
//! runs. `GaussClaw`'s equivalent inherits the safety story for free:
//!
//! 1. Every SOP carries a [`CapToken`](gauss_core::CapToken) grant
//!    declared at registration. The engine refuses to register an SOP
//!    whose grant the live kernel can't satisfy — symmetric with
//!    `PluginRegistry::register` (Sprint 7 §1).
//! 2. Every dispatch re-checks the workflow's required caps against
//!    the live grant at fire time — symmetric with
//!    [`gauss_cron::Scheduler::tick`]. A sub-agent that lost a cap
//!    between SOP definition and firing cannot run the workflow.
//! 3. Every run produces a [`SopRunReceipt`] whose digest is BLAKE3
//!    over the canonical event bytes + workflow outcome. The
//!    Sprint 14 §2 follow-on chains receipts into the existing
//!    `gauss-audit` Merkle log; this slice ships the receipt shape.
//!
//! ## Trait surface
//!
//! Three traits define the engine's plug-points:
//!
//! - [`Trigger`] — emits [`TriggerEvent`]s. The reference [
//!   `MemoryTrigger`] returns a pre-seeded vector for tests; a real
//!   webhook trigger lives in `gauss-sop`'s Sprint 14 §3 follow-on.
//! - [`Workflow`] — executes against a [`TriggerEvent`] and returns a
//!   [`WorkflowOutcome`]. The reference [`NoopWorkflow`] echoes the
//!   event into its outcome JSON for tests.
//! - [`ApprovalGate`] — optional pre-dispatch consent. The reference
//!   [`AlwaysApprove`] and [`AlwaysRefuse`] cover both ends of the
//!   spectrum; [`OperatorApprovalGate`] integration lands alongside
//!   the runtime surfaces in Sprint 14 §7.
//!
//! ## What this slice deliberately is *not*
//!
//! - Not a workflow scripting language. [`Workflow`] is a Rust trait;
//!   real workflows are typed steps. Adding Rhai / Lua / WASM is a
//!   reject-by-default decision per `docs/SPRINT_ZEROCLAW_SOP.md` §4.
//! - Not durable yet. Persistence to `SessionStore` lands in Sprint
//!   14 §2 — this slice ships the in-process engine + receipt shape so
//!   the trait surface can be reviewed without dragging the store
//!   into the dep graph.
//! - Not yet wired to `gaussclaw-channels` or `gauss-audit`. Those
//!   integrations are the §3 and §2 follow-ons, respectively.

#![allow(
    clippy::missing_docs_in_private_items,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

pub mod approval;
pub mod cancel;
pub mod engine;
pub mod error;
pub mod event;
pub mod receipt;
pub mod trigger;
pub mod workflow;

pub use approval::{AlwaysApprove, AlwaysRefuse, ApprovalDecision, ApprovalGate};
pub use cancel::CancelHandle;
pub use engine::{SopDef, SopEngine, SopId, WorkflowId};
pub use error::SopError;
pub use event::TriggerEvent;
pub use receipt::SopRunReceipt;
pub use trigger::{MemoryTrigger, Trigger};
pub use workflow::{NoopWorkflow, Workflow, WorkflowCtx, WorkflowOutcome};
