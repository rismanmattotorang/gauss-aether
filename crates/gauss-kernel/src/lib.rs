//! `gauss-kernel` — the privileged authority.
//!
//! Phase 0 ships:
//!
//! * The capability-lattice **type surface** with a working `meet`/`join`/`leq`
//!   on a small finite cap namespace.
//! * The three-plane scheduler skeleton with a lock-free token-bucket
//!   implementation (one bucket per plane). Phase 1 will lock A2/A4 and add
//!   the conformance test for Theorem T4 starvation freedom.
//! * Stubs (`unimplemented!`) for the joint-admissibility check, the `declass`
//!   map, and attestation — those land in Phases 1 and 4.
//!
//! See `SPECS.md` §4.

pub mod cap;
pub mod sched;

pub use cap::{CapToken, Capability};
pub use sched::{Plane, PlanePool, Planes, TokenBucket};
