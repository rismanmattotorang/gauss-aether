//! `gauss-kernel` — the privileged authority (Phase 1).
//!
//! Phase 1 ships:
//!
//! * Capability lattice **stabilised** at the Phase-0 surface (bitmask
//!   namespace from SPECS §4.1, full meet/join/leq with proptest coverage).
//! * **Information-flow lattice `L`** with declassification map (`declass`).
//!   Antitone check is enforced when registering a custom declass map.
//! * **Lock-free** three-plane token-bucket scheduler — single `AtomicU64`
//!   per plane encoding both tokens and last-refill timestamp, replacing the
//!   Phase-0 mutex.
//! * Joint capability/taint **admission** function realising paper §VI's
//!   `k ⪯ declass(ℓ) ⊓ Kt`.
//! * Concrete [`PrivilegedKernel`] implementing the [`gauss_traits::Kernel`]
//!   trait.
//!
//! Phase 1 LOCKS axioms A2 (capability monotonicity) and A4 (fairness
//! separation), and PROVES theorems T2 (capability non-interference) and T4
//! (plane starvation freedom) at the conformance level.
//!
//! See `SPECS.md` §4.

pub mod admit;
pub mod cap;
pub mod flow;
pub mod sched;

pub use admit::{declass_default, declass_strict, PrivilegedKernel};
pub use cap::{CapToken, Capability};
pub use flow::{default_declass, verify_antitone, DeclassMap};
pub use sched::{Plane, PlanePool, Planes, TokenBucket};
