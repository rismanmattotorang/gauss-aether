//! `gauss-audit` — Cryptographic Receipt Chain.
//!
//! Phase 2 shipped the un-signed SHA-256 chain `c_i = H(c_{i-1} ‖ payload_i)`.
//! Phase 5 layers signatures and external anchors on top **without changing**
//! the chain primitives:
//!
//! * [`chain`] — chain head + link + replay + inclusion-witness primitives.
//! * [`sign`] — Ed25519 [`SignedReceipt`] over each append, pluggable backend
//!   via [`SigningBackend`].
//! * [`tsa`] — RFC 3161 + `OpenTimestamps` anchor abstractions with an
//!   offline Ed25519 simulator ([`SimulatorTsaClient`]) so the conformance
//!   suite stays deterministic and network-free.
//! * [`anchor`] — cadence policy ([`AnchorPolicy::SPECS_DEFAULT`] = every
//!   1000 appends) and the [`Anchorer`] driver.
//! * [`verify`] — public verifier API surface for per-record, whole-chain,
//!   and anchor-replay checks.

pub mod anchor;
pub mod chain;
pub mod sign;
pub mod tsa;
pub mod verify;

// Phase-2 surface re-exports for ergonomic downstream usage.
pub use anchor::{AnchorPolicy, Anchorer};
pub use chain::{link, ChainHead, InclusionWitness, ReceiptChain, VerifyError};
pub use sign::{
    Ed25519Signer, ReceiptSigner, SignedReceipt, SigningBackend, ED25519_PUBLIC_KEY_LEN,
    ED25519_SECRET_KEY_LEN, ED25519_SIGNATURE_LEN,
};
pub use tsa::{Anchor, AnchorKind, SimulatorTsaClient, TsaClient};
pub use verify::{
    verify_anchor_replay, verify_chain, verify_receipt, verify_simulator_anchor,
    verifying_key_from_bytes,
};
