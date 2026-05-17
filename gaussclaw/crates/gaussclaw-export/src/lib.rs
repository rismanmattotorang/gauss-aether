//! `gaussclaw-export` — Phase 5 trajectory export plane.
//!
//! Replaces the upstream Hermes SFT / DPO JSONL writers with a Rust
//! implementation that preserves the Hermes field schema bit-for-bit
//! AND adds three guarantees Hermes has no equivalent for:
//!
//! 1. **Cryptographic Trajectory Envelope** ([`envelope`]). Every
//!    record is shipped alongside its signed receipt `ρᵢ`, the chain
//!    head `c_n` at envelope creation, a position witness `πᵢ` proving
//!    `ρᵢ` lives at chain index `i ≤ n`, and (optionally) a TSA anchor
//!    `TSA(c_n)`. A consumer that verifies the envelope obtains a
//!    cryptographic proof that the record came from a tamper-evident,
//!    wall-clock-anchored chain — without trusting the producer.
//!
//! 2. **Taint-Aware Filter** ([`filter`]). Three explicit modes
//!    selectable at export time:
//!    - `Permissive` — emit all records, taint marked in metadata.
//!    - `Strict` — drop records whose taint ⪰ Web.
//!    - `Declassified` — apply the runtime declass map; emit only
//!      records whose declassified taint ⪯ Trusted.
//!
//! 3. **End-to-end verifiable corpus** ([`verify::verify_envelope`]).
//!    A single static function checks: receipt signature is valid
//!    under the producer's published Ed25519 public key, payload
//!    digest binds the record bytes, post-head reconstructs from
//!    (prev_head, payload), position witness is consistent with the
//!    envelope's chain head, TSA anchor verifies under its authority.
//!    Hermes upstream has no equivalent.
//!
//! ## Hermes-superiorities (verified by tests in this crate)
//!
//! - **Schema parity.** [`sft::SftRecord`] and [`dpo::DpoRecord`]
//!   serialise to the exact JSONL field set Hermes emits. Byte-stable
//!   test corpora prove the diff is empty modulo timestamps.
//! - **Streaming writer.** [`sft::SftWriter`] / [`dpo::DpoWriter`]
//!   accept `&mut dyn AsyncWrite`; a deployment can stream straight to
//!   stdout, a file, an S3 multipart upload, or `gaussclaw-fed`'s
//!   federated pool without buffering. Hermes buffers in memory.
//! - **Filter is composable.** Filters are pure functions over the
//!   record + metadata; the same combinator stack runs in CI, in the
//!   federated subscriber, and in the desktop "Export → Filter →
//!   Preview" pane.
//! - **Envelope is optional.** Consumers that don't care can ignore
//!   the envelope; consumers that do can `verify_envelope` for free.
//!
//! ## Reference flow
//!
//! ```text
//!   SessionStore.list_session_turns()
//!     ↓
//!   into_sft_records()  (or into_dpo_pairs())
//!     ↓
//!   TaintFilter::apply()                   ← drops Web/Adversarial
//!     ↓
//!   EnvelopeBuilder::wrap_with(...)        ← attaches ρᵢ, c_n, πᵢ, TSA
//!     ↓
//!   SftWriter::write_envelope(...)         ← streams JSONL with envelope field
//!     ↓
//!   gaussclaw-fed publish_to_pool(...)     ← optional
//! ```

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::match_same_arms,
    clippy::large_enum_variant,
    clippy::single_match_else,
    clippy::drop_non_drop,
    clippy::iter_cloned_collect
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod dpo;
pub mod envelope;
pub mod filter;
pub mod sft;
pub mod verify;

pub use dpo::{into_dpo_pairs, DpoRecord, DpoWriter};
pub use envelope::{Envelope, EnvelopeBuilder, EnvelopeError, PositionWitness};
pub use filter::{FilterMode, FilterReport, TaintFilter};
pub use sft::{into_sft_records, SftMessage, SftRecord, SftWriter};
pub use verify::{verify_envelope, VerifyEnvelopeError};
