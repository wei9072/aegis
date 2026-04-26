use pyo3::prelude::*;

pub mod ast;
pub mod ir;
pub mod graph;
pub mod signals;
pub mod incremental;
pub mod enforcement;

// Keep signal_layer as the PyO3-facing aggregator
mod signal_layer_pyapi;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn _core(_py: Python, m: &PyModule) -> PyResult<()> {
    // Status
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;

    // AST layer
    m.add_class::<ast::parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast::parser::analyze_file, m)?)?;
    m.add_function(wrap_pyfunction!(ast::parser::get_imports, m)?)?;
    m.add_function(wrap_pyfunction!(ast::languages::typescript::extract_ts_imports, m)?)?;

    // IR layer
    m.add_class::<ir::model::IrNode>()?;
    m.add_function(wrap_pyfunction!(ir::builder::build_ir, m)?)?;

    // Graph layer
    m.add_class::<graph::DependencyGraph>()?;

    // Signals layer (Ring 0.5)
    m.add_class::<signal_layer_pyapi::Signal>()?;
    m.add_function(wrap_pyfunction!(signal_layer_pyapi::extract_signals, m)?)?;

    // Enforcement layer (Ring 0)
    m.add_function(wrap_pyfunction!(enforcement::check_syntax, m)?)?;

    // Incremental updater
    m.add_class::<incremental::IncrementalUpdater>()?;

    // V1.0 — register pure-Rust trace + decision types via pyshim.
    aegis_pyshim::register(m)?;

    Ok(())
}
