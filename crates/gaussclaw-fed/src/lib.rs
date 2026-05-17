//! `gaussclaw-fed` — Federated Trajectory Pool.
//!
//! Phase 5 §3 of `GAUSSCLAW_ROADMAP.md`. A small **publish / subscribe /
//! verify** service that consumes [`gaussclaw_export::Envelope`]s from
//! many producers and admits them into a shared pool only when:
//!
//! 1. The envelope's cryptographic surface fully verifies under the
//!    publisher's published Ed25519 public key + TSA trust root.
//! 2. The envelope's record passes a caller-supplied
//!    [`AdmissionPolicy`] (typically a chained taint filter +
//!    publisher allow list + max-taint cap).
//!
//! ## Pluggable backend
//!
//! The pool is parametric on a [`PoolBackend`] trait that defines
//! `put` / `list` / `get`. Two reference impls ship in-crate:
//!
//! - [`InMemoryPoolBackend`] — `Arc<Mutex<HashMap>>`-backed; used by
//!   tests and the desktop "Federated Preview" pane.
//! - [`FsPoolBackend`] *(behind a future `fs` feature)* — writes
//!   `<root>/<org>/<chain_head>/<turn_id>.env.json`. The S3-backed
//!   production backend lives in a follow-on crate so this base crate
//!   stays std-only and cross-platform.
//!
//! ## Object naming
//!
//! The roadmap pins the canonical object key:
//!
//! ```text
//!   {org}/{chain_head_hex}/{turn_id}.env.json
//! ```
//!
//! This buys two free properties:
//!
//! 1. **Content-addressed prefix.** `{chain_head_hex}` is the chain
//!    head at envelope creation. Two envelopes from the same producer
//!    at the same chain point share a prefix; consumers can stream a
//!    prefix scan to detect duplicates.
//! 2. **Tamper-evidence at the storage layer.** A storage backend that
//!    is itself tamper-evident (Merkle-tree-backed S3 bucket, or an
//!    IPFS UnixFS dag) inherits the chain's tamper-evidence for free.
//!
//! ## Hermes parity
//!
//! Hermes has **no federated pool** — local-only trajectory export.
//! GaussClaw's federated pool is the post-Hermes addition the paper
//! calls out as a GA prerequisite.

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::single_match_else,
    clippy::large_enum_variant,
    clippy::significant_drop_tightening
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod backend;
pub mod policy;
pub mod pool;

pub use backend::{InMemoryPoolBackend, PoolBackend, PoolEntry, PoolError, PoolResult};
pub use policy::{AdmissionDecision, AdmissionPolicy, MaxTaintPolicy, PublisherAllowList};
pub use pool::{FederatedPool, ObjectKey, PublishOutcome};
