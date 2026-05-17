//! `gaussclaw-providers-meta` — Phase 4 meta-router catalogue.
//!
//! Two routers ship in this crate:
//!
//! - [`OpenRouterProvider`] — aggregator. Holds a [`Catalogue`] plus a
//!   leaf provider per model. The router's `complete` dispatches by
//!   `prompt.model`; `route_complete` selects a candidate from the
//!   intersection of `candidates` and the catalogue, then delegates.
//!   The selected model is recorded in [`RoutedCompletion::selected`]
//!   for the receipt chain.
//!
//! - [`NotDiamondProvider`] — learned router (advisory mode). Takes a
//!   candidate set, picks one model via a pluggable
//!   [`SelectionStrategy`], and delegates. The Phase-4 baseline
//!   strategy is `FirstCandidateStrategy`; a real ML-trained
//!   strategy plugs in via the same trait.
//!
//! Both routers satisfy the **router-transparency** post-condition
//! from `gaussclaw_providers::router::check_transparency`: the routed
//! completion's `text` + `finish_reason` are exactly what the leaf
//! provider would have produced under a direct call.
//!
//! ## Why these matter (Hermes baseline)
//!
//! Hermes upstream has no router abstraction. Users wire their own
//! retry / fallback logic per call site. GaussClaw moves this into a
//! typed trait surface so:
//!
//! 1. Fallback chains compile only when members are polyhedrally
//!    equivalent on the working set (Phase 4 slice 5 builder).
//! 2. The kernel filters the catalogue before the router sees it
//!    (capability lower bound) — a router cannot dispatch to a model
//!    the kernel would have refused.
//! 3. Routed turns produce receipts that record both the candidate set
//!    and the actually-chosen model, so the routing decision is
//!    auditable.

#![allow(
    clippy::doc_markdown,
    clippy::len_zero,
    clippy::assigning_clones,
    clippy::map_unwrap_or,
)]

pub mod notdiamond;
pub mod openrouter;

pub use gaussclaw_providers::{Catalogue, LeafModel, RoutedCompletion, RouterProvider};
pub use notdiamond::{FirstCandidateStrategy, NotDiamondProvider, SelectionStrategy};
pub use openrouter::OpenRouterProvider;
