//! PyO3 wrappers for `aegis-runtime` primitives.
//!
//! V1.2 / V1.3 surfaces:
//!
//!   - `Snapshot` — file-set capture + restore (language-agnostic
//!     IO primitive; lives in `aegis-runtime::snapshot`)
//!   - `is_state_stalemate` / `is_thrashing` /
//!     `is_plan_repeat_stalemate` — Gap 1 detector helpers
//!   - `Executor` + `ExecutionResult` + `PatchResult` — the V1.2
//!     class port (atomic plan apply with backup + rollback)
//!   - `PlanValidator` + `ValidationError` — the V1.2 class port
//!     (gate between Planner and Executor; schema, path-safety,
//!     scope, simulation)
//!
//! Rust is the ground-truth implementation; Python
//! `aegis.runtime.executor` / `aegis.runtime.validator` re-export
//! from `aegis._core`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aegis_runtime::{
    is_plan_repeat_stalemate as rs_plan_repeat,
    is_state_stalemate as rs_state_stalemate, is_thrashing as rs_thrashing,
    ExecutionResult as RsExecutionResult, Executor as RsExecutor,
    PatchResult as RsPatchResult, PlanValidator as RsPlanValidator, Snapshot,
    ValidationError as RsValidationError,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::ir::{patchstatus_to_py, PyPatchPlan, PyPatchStatus};

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

// ---------- Executor + ExecutionResult + PatchResult ----------

#[pyclass(name = "PatchResult", module = "aegis._core")]
#[derive(Clone)]
pub struct PyPatchResult {
    inner: RsPatchResult,
}

#[pymethods]
impl PyPatchResult {
    #[getter]
    fn patch_id(&self) -> &str {
        &self.inner.patch_id
    }

    #[getter]
    fn status(&self) -> PyPatchStatus {
        patchstatus_to_py(self.inner.status)
    }

    #[getter]
    fn matches(&self) -> usize {
        self.inner.matches
    }

    #[getter]
    fn error(&self) -> Option<&str> {
        self.inner.error.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "PatchResult(patch_id={:?}, status=PatchStatus.{}, matches={}, error={:?})",
            self.inner.patch_id,
            self.inner.status.as_str(),
            self.inner.matches,
            self.inner.error
        )
    }
}

#[pyclass(name = "ExecutionResult", module = "aegis._core")]
#[derive(Clone)]
pub struct PyExecutionResult {
    inner: RsExecutionResult,
}

impl PyExecutionResult {
    pub(crate) fn from_inner(inner: RsExecutionResult) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyExecutionResult {
    #[new]
    #[pyo3(signature = (
        success=false,
        results=None,
        backup_dir=None,
        rolled_back=false,
        staleness_detected=false,
        created_paths=None,
        touched_paths=None,
        path_contents=None,
    ))]
    fn new(
        success: bool,
        results: Option<&PyList>,
        backup_dir: Option<String>,
        rolled_back: bool,
        staleness_detected: bool,
        created_paths: Option<&PyList>,
        touched_paths: Option<&PyList>,
        path_contents: Option<&PyDict>,
    ) -> PyResult<Self> {
        let results = match results {
            Some(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let pr: PyPatchResult = item.extract()?;
                    out.push(pr.inner);
                }
                out
            }
            None => Vec::new(),
        };
        let to_str_vec = |list: Option<&PyList>| -> PyResult<Vec<String>> {
            let mut out = Vec::new();
            if let Some(l) = list {
                for item in l.iter() {
                    out.push(item.extract::<String>()?);
                }
            }
            Ok(out)
        };
        let mut path_contents_btree = BTreeMap::new();
        if let Some(d) = path_contents {
            for (k, v) in d.iter() {
                path_contents_btree.insert(k.extract::<String>()?, v.extract::<String>()?);
            }
        }
        Ok(Self {
            inner: RsExecutionResult {
                success,
                results,
                backup_dir,
                rolled_back,
                staleness_detected,
                created_paths: to_str_vec(created_paths)?,
                touched_paths: to_str_vec(touched_paths)?,
                path_contents: path_contents_btree,
            },
        })
    }

    #[getter]
    fn success(&self) -> bool {
        self.inner.success
    }

    #[getter]
    fn results(&self) -> Vec<PyPatchResult> {
        self.inner
            .results
            .iter()
            .cloned()
            .map(|inner| PyPatchResult { inner })
            .collect()
    }

    #[getter]
    fn backup_dir(&self) -> Option<&str> {
        self.inner.backup_dir.as_deref()
    }

    #[getter]
    fn rolled_back(&self) -> bool {
        self.inner.rolled_back
    }

    #[getter]
    fn staleness_detected(&self) -> bool {
        self.inner.staleness_detected
    }

    #[getter]
    fn created_paths(&self) -> Vec<String> {
        self.inner.created_paths.clone()
    }

    #[getter]
    fn touched_paths(&self) -> Vec<String> {
        self.inner.touched_paths.clone()
    }

    #[getter]
    fn path_contents<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        let d = PyDict::new(py);
        for (k, v) in &self.inner.path_contents {
            d.set_item(k, v)?;
        }
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "ExecutionResult(success={}, rolled_back={}, results={} items, touched={} paths)",
            self.inner.success,
            self.inner.rolled_back,
            self.inner.results.len(),
            self.inner.touched_paths.len()
        )
    }
}

#[pyclass(name = "Executor", module = "aegis._core")]
pub struct PyExecutor {
    inner: RsExecutor,
}

#[pymethods]
impl PyExecutor {
    #[new]
    #[pyo3(signature = (
        root,
        backup_subdir=".aegis/backups".to_string(),
        keep_backups=5_usize
    ))]
    fn new(root: PathBuf, backup_subdir: String, keep_backups: usize) -> Self {
        let inner = RsExecutor::new(root)
            .with_backup_subdir(backup_subdir)
            .with_keep_backups(keep_backups);
        Self { inner }
    }

    fn apply(&self, plan: &PyPatchPlan) -> PyExecutionResult {
        let result = self.inner.apply(plan.inner_ref());
        PyExecutionResult::from_inner(result)
    }

    fn rollback_result(&self, result: &PyExecutionResult) -> PyResult<()> {
        self.inner
            .rollback_result(&result.inner)
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))
    }

    #[getter]
    fn root(&self) -> PathBuf {
        self.inner.root.clone()
    }

    #[getter]
    fn backup_subdir(&self) -> &str {
        &self.inner.backup_subdir
    }

    #[getter]
    fn keep_backups(&self) -> usize {
        self.inner.keep_backups
    }
}

// ---------- PlanValidator + ValidationError ----------

#[pyclass(name = "ValidationError", module = "aegis._core")]
#[derive(Clone)]
pub struct PyValidationError {
    inner: RsValidationError,
}

#[pymethods]
impl PyValidationError {
    #[new]
    #[pyo3(signature = (
        kind,
        message,
        patch_id=None,
        edit_index=None,
        matches=0_usize
    ))]
    fn new(
        kind: String,
        message: String,
        patch_id: Option<String>,
        edit_index: Option<usize>,
        matches: usize,
    ) -> PyResult<Self> {
        let kind = aegis_runtime::ErrorKind::from_str(&kind)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown ErrorKind {kind}")))?;
        Ok(Self {
            inner: RsValidationError {
                kind,
                message,
                patch_id,
                edit_index,
                matches,
            },
        })
    }

    /// Python tests pattern-match `e.kind == "path"` etc., so the
    /// kind getter returns the lowercase string the V0.x `Literal`
    /// type used. The Rust `ErrorKind` enum is internal.
    #[getter]
    fn kind(&self) -> &'static str {
        self.inner.kind.as_str()
    }

    #[getter]
    fn message(&self) -> &str {
        &self.inner.message
    }

    #[getter]
    fn patch_id(&self) -> Option<&str> {
        self.inner.patch_id.as_deref()
    }

    #[getter]
    fn edit_index(&self) -> Option<usize> {
        self.inner.edit_index
    }

    #[getter]
    fn matches(&self) -> usize {
        self.inner.matches
    }

    fn __repr__(&self) -> String {
        format!(
            "ValidationError(kind={:?}, message={:?}, patch_id={:?}, edit_index={:?})",
            self.inner.kind.as_str(),
            self.inner.message,
            self.inner.patch_id,
            self.inner.edit_index
        )
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyValidationError>() {
            return self.inner == other.inner;
        }
        false
    }
}

#[pyclass(name = "PlanValidator", module = "aegis._core")]
pub struct PyPlanValidator {
    inner: RsPlanValidator,
}

#[pymethods]
impl PyPlanValidator {
    #[new]
    #[pyo3(signature = (root, scope=None))]
    fn new(root: PathBuf, scope: Option<&PyList>) -> PyResult<Self> {
        let mut v = RsPlanValidator::new(root);
        if let Some(scope) = scope {
            let scope: Vec<String> = scope
                .iter()
                .map(|item| item.extract::<String>())
                .collect::<PyResult<_>>()?;
            v = v
                .with_scope(scope)
                .map_err(pyo3::exceptions::PyValueError::new_err)?;
        }
        Ok(Self { inner: v })
    }

    fn validate(&self, plan: &PyPatchPlan) -> Vec<PyValidationError> {
        self.inner
            .validate(plan.inner_ref())
            .into_iter()
            .map(|inner| PyValidationError { inner })
            .collect()
    }

    #[getter]
    fn root(&self) -> PathBuf {
        self.inner.root.clone()
    }
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PySnapshot>()?;
    m.add_class::<PyExecutor>()?;
    m.add_class::<PyExecutionResult>()?;
    m.add_class::<PyPatchResult>()?;
    m.add_class::<PyPlanValidator>()?;
    m.add_class::<PyValidationError>()?;
    m.add_function(wrap_pyfunction!(is_state_stalemate, m)?)?;
    m.add_function(wrap_pyfunction!(is_thrashing, m)?)?;
    m.add_function(wrap_pyfunction!(is_plan_repeat_stalemate, m)?)?;
    Ok(())
}
