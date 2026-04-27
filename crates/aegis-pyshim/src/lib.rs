//! `aegis._core` Python extension — V1.9 cdylib.
//!
//! Hosts every PyO3 wrapper Aegis exposes to Python. Pre-V1.9 this
//! crate was a sibling rlib called from `aegis-core`'s `_core` module;
//! V1.9's cycle-break moved the cdylib boundary here so this crate
//! can depend on `aegis-core` for build_context + planner ports.
//!
//! Wraps everything Python imports as `aegis._core.<Name>`:
//!   - DecisionTrace / DecisionEvent + 4 verb constants
//!   - DecisionPattern + derive_pattern + IterationEvent
//!   - TaskPattern / TaskVerdict / TaskVerifier
//!   - PatchKind / PatchStatus / Edit / Patch / PatchPlan + edit engine
//!   - LLM provider (Rust OpenAIChat impl)
//!   - Snapshot / Executor / PlanValidator + metrics + run_loop
//!   - Ring 0 / Ring 0.5 functions from aegis-core (analyze_file,
//!     get_imports, extract_signals, …)
//!
//! At V1.10 this crate's PyO3 surface goes away, and the underlying
//! Rust crates ship as a CLI binary.

mod context;
mod trace;
mod decision;
mod ir;
mod task;
mod providers;
mod runtime;
mod pipeline;

use pyo3::prelude::*;

/// Public entry point — kept so external code that calls
/// `aegis_pyshim::register(m)` (e.g. an embedder hosting the
/// extension manually) keeps working. The `#[pymodule]` below also
/// invokes it.
pub fn register(m: &PyModule) -> PyResult<()> {
    trace::register(m)?;
    decision::register(m)?;
    ir::register(m)?;
    task::register(m)?;
    providers::register(m)?;
    runtime::register(m)?;
    pipeline::register(m)?;
    context::register(m)?;
    Ok(())
}

// ---------- aegis-core PyO3 surface (moved from aegis-core/lib.rs) ----------

use aegis_core::ast;
use aegis_core::enforcement;
use aegis_core::graph;
use aegis_core::incremental;
use aegis_core::ir as ircore;
use aegis_core::signal_layer_pyapi;
use aegis_core::{ring0_status, supported_extensions, supported_languages};

#[pymodule]
fn _core(_py: Python, m: &PyModule) -> PyResult<()> {
    // Status + registry introspection
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_function(wrap_pyfunction!(supported_languages, m)?)?;
    m.add_function(wrap_pyfunction!(supported_extensions, m)?)?;

    // AST layer
    m.add_class::<ast::parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast::parser::analyze_file, m)?)?;
    m.add_function(wrap_pyfunction!(ast::parser::get_imports, m)?)?;
    m.add_function(wrap_pyfunction!(
        ast::languages::typescript::extract_ts_imports,
        m
    )?)?;

    // IR layer
    m.add_class::<ircore::model::IrNode>()?;
    m.add_function(wrap_pyfunction!(ircore::builder::build_ir, m)?)?;

    // Graph layer
    m.add_class::<graph::DependencyGraph>()?;

    // Signals layer (Ring 0.5)
    m.add_class::<signal_layer_pyapi::Signal>()?;
    m.add_function(wrap_pyfunction!(signal_layer_pyapi::extract_signals, m)?)?;

    // Enforcement layer (Ring 0)
    m.add_function(wrap_pyfunction!(enforcement::check_syntax, m)?)?;

    // Incremental updater
    m.add_class::<incremental::IncrementalUpdater>()?;

    // V1.0+ — pure-Rust types from sibling crates, registered through
    // the per-module `register` functions in this crate.
    register(m)?;

    Ok(())
}
