//! Engine-side error type.

use thiserror::Error;

/// SOP engine error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SopError {
    /// The SOP being registered (or fired) requires caps the live
    /// grant doesn't satisfy. The engine refuses closed; no workflow
    /// runs, no receipt advances.
    #[error("admit refused: required caps 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Cap bits required by the SOP definition.
        required: u64,
        /// Cap bits the live kernel grant currently exposes.
        grant: u64,
    },
    /// Two SOPs registered under the same id.
    #[error("sop already registered: {0}")]
    Duplicate(String),
    /// Looked up an SOP that isn't registered.
    #[error("sop not found: {0}")]
    NotFound(String),
    /// A workflow returned an internal error.
    #[error("workflow failed: {0}")]
    Workflow(String),
}
