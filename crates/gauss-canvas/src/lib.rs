//! `gauss-canvas` — A2UI Live Canvas Protocol (paper §XIII.A).
//!
//! Phase 9 ships the **typed widget tree** + **update stream** that
//! human-facing surfaces (web, TUI, IDE plugins) subscribe to. The canvas
//! is content-addressed: every node has a stable `NodeId` so the surface
//! reconciles updates with a Merkle-style replay (no diff calculation on
//! the wire).
//!
//! The crate is intentionally thin in Phase 9 — the live-query
//! integration over `SurrealDB` lands as an additive feature in Phase 10's
//! cluster mode. Phase 9 ships:
//!
//! * [`WidgetKind`] enumeration covering the core widgets named in SPECS
//!   §XIII.B (text, button, table, image, approval prompt, etc.).
//! * [`CanvasNode`] — typed widget tree node.
//! * [`CanvasUpdate`] — `Insert` / `Update` / `Delete` / `SetProp`
//!   commands.
//! * [`Canvas`] trait — async surface for backends.
//! * [`InMemoryCanvas`] — deterministic in-process impl for tests and
//!   the `gauss doctor` health surface.
//!
//! ADR-0015 documents the widget-set freeze and the Phase-10 streaming
//! migration.

pub mod canvas;
pub mod node;

pub use canvas::{Canvas, CanvasError, InMemoryCanvas};
pub use node::{CanvasNode, CanvasUpdate, NodeId, WidgetKind};
