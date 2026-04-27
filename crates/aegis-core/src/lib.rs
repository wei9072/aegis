//! `aegis-core` — Ring 0 / Ring 0.5 analysis primitives.
//!
//! Pure-rlib crate. The Python extension (`aegis._core` cdylib) lives
//! in `aegis-pyshim` since the V1.9 cycle-break — this crate exposes
//! the analysis APIs as plain Rust functions; the wrappers in
//! `aegis-pyshim/src/lib.rs` register them into the `_core` PyO3
//! module.

use pyo3::prelude::*;

pub mod ast;
pub mod ir;
pub mod graph;
pub mod signals;
pub mod incremental;
pub mod enforcement;

// Keep signal_layer as the PyO3-facing aggregator; pub so
// aegis-pyshim can register the Signal class + extract_signals fn.
pub mod signal_layer_pyapi;

#[pyfunction]
pub fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pyfunction]
pub fn supported_languages() -> Vec<&'static str> {
    ast::registry::LanguageRegistry::global().names()
}

#[pyfunction]
pub fn supported_extensions() -> Vec<&'static str> {
    ast::registry::LanguageRegistry::global().extensions()
}
