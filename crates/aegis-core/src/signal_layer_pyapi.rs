use pyo3::prelude::*;
use crate::signals::{fan_out_signal, chain_depth_signal};

#[pyclass(get_all)]
#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub value: f64,
    pub file_path: String,
    pub description: String,
}

#[pymethods]
impl Signal {
    #[new]
    pub fn new(name: String, value: f64, file_path: String, description: String) -> Self {
        Signal { name, value, file_path, description }
    }

    pub fn __repr__(&self) -> String {
        format!("Signal({} = {} @ {})", self.name, self.value, self.file_path)
    }
}

/// Pure-data signal record (no PyO3 dependency). Exposed so V1.9
/// binaries that link `aegis-core` directly can read signals
/// without dragging in the Python extension scaffolding.
#[derive(Clone, Debug)]
pub struct SignalData {
    pub name: String,
    pub value: f64,
    pub file_path: String,
    pub description: String,
}

#[pyfunction]
pub fn extract_signals(filepath: &str) -> PyResult<Vec<Signal>> {
    let data = extract_signals_native(filepath)
        .map_err(pyo3::exceptions::PyIOError::new_err)?;
    Ok(data
        .into_iter()
        .map(|d| Signal::new(d.name, d.value, d.file_path, d.description))
        .collect())
}

/// Pure-Rust signal extraction. Same content as `extract_signals`
/// but returns `Vec<SignalData>` (no PyO3 types).
pub fn extract_signals_native(filepath: &str) -> Result<Vec<SignalData>, String> {
    let fan_out = fan_out_signal(filepath)?;
    let depth = chain_depth_signal(filepath)?;

    Ok(vec![
        SignalData {
            name: "fan_out".to_string(),
            value: fan_out,
            file_path: filepath.to_string(),
            description: format!(
                "Number of unique external imports (fan-out = {})",
                fan_out as usize
            ),
        },
        SignalData {
            name: "max_chain_depth".to_string(),
            value: depth,
            file_path: filepath.to_string(),
            description: format!(
                "Maximum method/attribute chain depth (depth = {})",
                depth as usize
            ),
        },
    ])
}
