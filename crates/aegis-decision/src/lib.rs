//! Layer B + Layer C primitives.
//!
//! - `DecisionPattern` + `derive_pattern`  — per-iteration shape
//! - `TaskPattern` + `TaskVerdict` + `TaskVerifier`
//!   — per-task outcome
//!
//! Pure data + a single trait. PyO3 wrappers live in `aegis-pyshim`.
//! The Critical Principle (verifier output never feeds back into the
//! loop) is enforced structurally: there is no field on `TaskVerdict`
//! that the loop could read, and `TaskVerifier::verify` is the only
//! method on the trait.

pub mod iteration;
pub mod pattern;
pub mod task;

pub use iteration::IterationEvent;
pub use pattern::{derive_pattern, DecisionPattern};
pub use task::{
    apply_verifier, derive_task_pattern, TaskPattern, TaskVerdict, TaskVerifier, VerifierResult,
};
