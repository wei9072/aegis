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

#[pyfunction]
pub fn extract_signals(filepath: &str) -> PyResult<Vec<Signal>> {
    let fan_out = fan_out_signal(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e))?;
    let depth = chain_depth_signal(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e))?;

    Ok(vec![
        Signal::new(
            "fan_out".to_string(),
            fan_out,
            filepath.to_string(),
            format!("Number of unique external imports (fan-out = {})", fan_out as usize),
        ),
        Signal::new(
            "max_chain_depth".to_string(),
            depth,
            filepath.to_string(),
            format!("Maximum method/attribute chain depth (depth = {})", depth as usize),
        ),
    ])
}
