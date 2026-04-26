//! PyO3 wrappers for `aegis-runtime` primitives.
//!
//! Two surfaces in V1.2 / V1.3:
//!
//!   - `Snapshot` — file-set capture + restore (mirrors V0.x
//!     `Executor` snapshot semantics, language-agnostic)
//!   - `is_state_stalemate` / `is_thrashing` /
//!     `is_plan_repeat_stalemate` — Gap 1 detector helpers
//!     (re-exports of the pure-Rust functions)
//!
//! The Python pipeline still runs the loop. These functions are
//! the ground-truth implementations Python calls; if anyone moves
//! the loop to Rust later (post-V2), it calls the same functions
//! with no behaviour delta.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aegis_runtime::{
    is_plan_repeat_stalemate as rs_plan_repeat,
    is_state_stalemate as rs_state_stalemate, is_thrashing as rs_thrashing, Snapshot,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

#[pyclass(name = "Snapshot", module = "aegis._core")]
pub struct PySnapshot {
    inner: std::sync::Mutex<Snapshot>,
}

#[pymethods]
impl PySnapshot {
    #[new]
    fn new(root: PathBuf) -> Self {
        Self {
            inner: std::sync::Mutex::new(Snapshot::new(root)),
        }
    }

    /// Snapshot every path in `rel_paths` (relative to root).
    /// Idempotent — already-snapshotted paths are skipped.
    fn capture(&self, rel_paths: &PyList) -> PyResult<()> {
        let mut snap = self.inner.lock().unwrap();
        for item in rel_paths.iter() {
            let rel: String = item.extract()?;
            snap.add(rel)
                .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
        }
        Ok(())
    }

    fn restore(&self) -> PyResult<()> {
        let snap = self.inner.lock().unwrap();
        snap.restore()
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
    }

    fn write_backup(&self, backup_dir: PathBuf) -> PyResult<()> {
        let snap = self.inner.lock().unwrap();
        snap.write_backup(&backup_dir)
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
    }

    #[getter]
    fn touched_paths(&self) -> Vec<String> {
        self.inner.lock().unwrap().touched_paths()
    }

    #[getter]
    fn created_paths(&self) -> Vec<String> {
        self.inner.lock().unwrap().created_paths()
    }

    #[getter]
    fn root(&self) -> PathBuf {
        self.inner.lock().unwrap().root().to_path_buf()
    }

    fn __len__(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

fn dict_to_btree(d: Option<&PyDict>) -> PyResult<BTreeMap<String, f64>> {
    let mut out = BTreeMap::new();
    if let Some(d) = d {
        for (k, v) in d.iter() {
            let k: String = k.extract()?;
            let v: f64 = v.extract()?;
            out.insert(k, v);
        }
    }
    Ok(out)
}

fn list_to_btree_vec(items: Option<&PyList>) -> PyResult<Vec<BTreeMap<String, f64>>> {
    let mut out = Vec::new();
    if let Some(items) = items {
        for item in items.iter() {
            let d: &PyDict = item.downcast()?;
            out.push(dict_to_btree(Some(d))?);
        }
    }
    Ok(out)
}

#[pyfunction]
#[pyo3(signature = (history, current_value_totals))]
pub fn is_state_stalemate(
    history: Option<&PyList>,
    current_value_totals: &PyDict,
) -> PyResult<bool> {
    let h = list_to_btree_vec(history)?;
    let c = dict_to_btree(Some(current_value_totals))?;
    Ok(rs_state_stalemate(&h, &c))
}

#[pyfunction]
#[pyo3(signature = (history, regressed_now))]
pub fn is_thrashing(history: Option<&PyList>, regressed_now: bool) -> PyResult<bool> {
    let mut h = Vec::new();
    if let Some(items) = history {
        for item in items.iter() {
            h.push(item.extract::<bool>()?);
        }
    }
    Ok(rs_thrashing(&h, regressed_now))
}

#[pyfunction]
#[pyo3(signature = (plan_repeated_now, value_totals_history, current_value_totals))]
pub fn is_plan_repeat_stalemate(
    plan_repeated_now: bool,
    value_totals_history: Option<&PyList>,
    current_value_totals: &PyDict,
) -> PyResult<bool> {
    let h = list_to_btree_vec(value_totals_history)?;
    let c = dict_to_btree(Some(current_value_totals))?;
    Ok(rs_plan_repeat(plan_repeated_now, &h, &c))
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PySnapshot>()?;
    m.add_function(wrap_pyfunction!(is_state_stalemate, m)?)?;
    m.add_function(wrap_pyfunction!(is_thrashing, m)?)?;
    m.add_function(wrap_pyfunction!(is_plan_repeat_stalemate, m)?)?;
    Ok(())
}
