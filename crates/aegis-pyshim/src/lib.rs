//! PyO3 wrappers for `aegis-trace` + `aegis-decision`.
//!
//! Exposes `register(m)` that the maturin extension entry point in
//! `aegis-core::lib::_core` calls during module init. Keeps the
//! single-`.so` install layout from V0.x — `aegis._core.<name>`.
//!
//! V1.0 wraps:
//!   - DecisionEvent / DecisionTrace + 4 verb constants
//!   - DecisionPattern enum + derive_pattern (consumes a duck-typed
//!     Python object that exposes the IterationEvent attribute set)
//!   - TaskPattern / VerifierResult / TaskVerdict + derive_task_pattern
//!     + apply_verifier (verifier supplied as a Python object with
//!     a `verify(workspace, trace) -> VerifierResult` method)
//!
//! At V1.10 this whole crate disappears.

mod trace;
mod decision;
mod task;
mod providers;

use pyo3::prelude::*;

/// Add every wrapper to the supplied module. Called once during
/// `_core` module init. Order is irrelevant; classes register
/// independent of one another.
pub fn register(m: &PyModule) -> PyResult<()> {
    trace::register(m)?;
    decision::register(m)?;
    task::register(m)?;
    providers::register(m)?;
    Ok(())
}
