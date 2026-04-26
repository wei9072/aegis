//! PyO3 wrapper around the Layer C primitives.
//!
//! The verifier crossing the Rust/Python boundary uses Python-side
//! verifiers (V0.x verifier impls live in `tests/scenarios/`). The
//! Rust adapter reads `verify(workspace, trace)` from any Python
//! object via duck typing — same as the Python `TaskVerifier`
//! Protocol.

use std::collections::HashMap;
use std::path::PathBuf;

use aegis_decision::{
    apply_verifier as rs_apply, derive_task_pattern as rs_derive_task, IterationEvent, TaskPattern,
    TaskVerdict, TaskVerifier, VerifierResult,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

use crate::trace::{json_to_py, py_to_json};

#[pyclass(name = "TaskPattern", module = "aegis._core")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PyTaskPattern {
    #[pyo3(name = "SOLVED")]
    Solved,
    #[pyo3(name = "INCOMPLETE")]
    Incomplete,
    #[pyo3(name = "ABANDONED")]
    Abandoned,
    #[pyo3(name = "NO_VERIFIER")]
    NoVerifier,
    #[pyo3(name = "VERIFIER_ERROR")]
    VerifierError,
}

impl From<TaskPattern> for PyTaskPattern {
    fn from(p: TaskPattern) -> Self {
        match p {
            TaskPattern::Solved => Self::Solved,
            TaskPattern::Incomplete => Self::Incomplete,
            TaskPattern::Abandoned => Self::Abandoned,
            TaskPattern::NoVerifier => Self::NoVerifier,
            TaskPattern::VerifierError => Self::VerifierError,
        }
    }
}

impl From<PyTaskPattern> for TaskPattern {
    fn from(p: PyTaskPattern) -> Self {
        match p {
            PyTaskPattern::Solved => Self::Solved,
            PyTaskPattern::Incomplete => Self::Incomplete,
            PyTaskPattern::Abandoned => Self::Abandoned,
            PyTaskPattern::NoVerifier => Self::NoVerifier,
            PyTaskPattern::VerifierError => Self::VerifierError,
        }
    }
}

#[pymethods]
impl PyTaskPattern {
    #[getter]
    fn value(&self) -> &'static str {
        TaskPattern::from(*self).as_str()
    }

    fn __str__(&self) -> &'static str {
        self.value()
    }

    fn __repr__(&self) -> String {
        format!("TaskPattern.{}", self.name())
    }

    fn __hash__(&self) -> isize {
        *self as isize
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyTaskPattern>() {
            return *self == other;
        }
        if let Ok(s) = other.extract::<String>() {
            return self.value() == s;
        }
        false
    }

    fn __ne__(&self, other: &PyAny) -> bool {
        !self.__eq__(other)
    }

    #[getter]
    fn name(&self) -> &'static str {
        match self {
            Self::Solved => "SOLVED",
            Self::Incomplete => "INCOMPLETE",
            Self::Abandoned => "ABANDONED",
            Self::NoVerifier => "NO_VERIFIER",
            Self::VerifierError => "VERIFIER_ERROR",
        }
    }

    #[classmethod]
    fn from_value(_cls: &PyType, s: &str) -> PyResult<Self> {
        TaskPattern::from_str(s).map(Self::from).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("unknown task pattern {s}"))
        })
    }

    #[classmethod]
    fn members(_cls: &PyType) -> Vec<Self> {
        vec![
            Self::Solved,
            Self::Incomplete,
            Self::Abandoned,
            Self::NoVerifier,
            Self::VerifierError,
        ]
    }
}

#[pyclass(name = "VerifierResult", module = "aegis._core", frozen)]
#[derive(Clone)]
pub struct PyVerifierResult {
    inner: VerifierResult,
}

#[pymethods]
impl PyVerifierResult {
    #[new]
    #[pyo3(signature = (passed, rationale="".to_string(), evidence=None))]
    fn new(passed: bool, rationale: String, evidence: Option<&PyDict>) -> PyResult<Self> {
        let mut ev = HashMap::new();
        if let Some(d) = evidence {
            for (k, v) in d.iter() {
                let k: String = k.extract()?;
                ev.insert(k, py_to_json(v)?);
            }
        }
        Ok(Self {
            inner: VerifierResult {
                passed,
                rationale,
                evidence: ev,
            },
        })
    }

    #[getter]
    fn passed(&self) -> bool {
        self.inner.passed
    }

    #[getter]
    fn rationale(&self) -> &str {
        &self.inner.rationale
    }

    #[getter]
    fn evidence<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        let d = PyDict::new(py);
        for (k, v) in &self.inner.evidence {
            d.set_item(k, json_to_py(py, v)?)?;
        }
        Ok(d)
    }
}

#[pyclass(name = "TaskVerdict", module = "aegis._core", frozen)]
#[derive(Clone)]
pub struct PyTaskVerdict {
    inner: TaskVerdict,
}

#[pymethods]
impl PyTaskVerdict {
    #[new]
    #[pyo3(signature = (pattern, verifier_result, pipeline_done, iterations_run, error="".to_string()))]
    fn new(
        pattern: PyTaskPattern,
        verifier_result: Option<PyVerifierResult>,
        pipeline_done: bool,
        iterations_run: u32,
        error: String,
    ) -> Self {
        Self {
            inner: TaskVerdict {
                pattern: pattern.into(),
                verifier_result: verifier_result.map(|v| v.inner),
                pipeline_done,
                iterations_run,
                error,
            },
        }
    }

    #[getter]
    fn pattern(&self) -> PyTaskPattern {
        self.inner.pattern.into()
    }

    #[getter]
    fn verifier_result(&self) -> Option<PyVerifierResult> {
        self.inner
            .verifier_result
            .clone()
            .map(|inner| PyVerifierResult { inner })
    }

    #[getter]
    fn pipeline_done(&self) -> bool {
        self.inner.pipeline_done
    }

    #[getter]
    fn iterations_run(&self) -> u32 {
        self.inner.iterations_run
    }

    #[getter]
    fn error(&self) -> &str {
        &self.inner.error
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        let d = PyDict::new(py);
        d.set_item("pattern", self.inner.pattern.as_str())?;
        d.set_item("pipeline_done", self.inner.pipeline_done)?;
        d.set_item("iterations_run", self.inner.iterations_run)?;
        d.set_item("error", &self.inner.error)?;
        match &self.inner.verifier_result {
            None => d.set_item("verifier_result", py.None())?,
            Some(r) => {
                let inner = PyDict::new(py);
                inner.set_item("passed", r.passed)?;
                inner.set_item("rationale", &r.rationale)?;
                let ev = PyDict::new(py);
                for (k, v) in &r.evidence {
                    ev.set_item(k, json_to_py(py, v)?)?;
                }
                inner.set_item("evidence", ev)?;
                d.set_item("verifier_result", inner)?;
            }
        }
        Ok(d)
    }
}

#[pyfunction]
#[pyo3(signature = (verifier_present, verifier_passed, verifier_raised, pipeline_done))]
pub fn derive_task_pattern(
    verifier_present: bool,
    verifier_passed: Option<bool>,
    verifier_raised: bool,
    pipeline_done: bool,
) -> PyTaskPattern {
    rs_derive_task(verifier_present, verifier_passed, verifier_raised, pipeline_done).into()
}

/// Bridge: a Python verifier object adapted to the Rust trait. The
/// Python side raises an exception → we surface it as a verdict.
struct PyVerifier {
    inner: PyObject,
}

impl TaskVerifier for PyVerifier {
    fn verify(
        &self,
        workspace: &std::path::Path,
        _trace: &[IterationEvent],
    ) -> VerifierResult {
        // The Python verifier expects pathlib.Path + the original
        // Python trace list — both built fresh on each call. The
        // Rust IterationEvent slice is empty here in V1.0 because
        // Python is still the source of truth for the loop; the
        // adapter passes through the original Python trace via the
        // `apply_verifier` Python wrapper instead.
        Python::with_gil(|py| {
            let path = PathBuf::from(workspace);
            let path_obj: PyObject = path
                .to_str()
                .map(|s| s.to_object(py))
                .unwrap_or_else(|| py.None());
            let trace = PyList::empty(py);
            let r = self
                .inner
                .as_ref(py)
                .call_method1("verify", (path_obj, trace));
            match r {
                Ok(result) => {
                    let passed = result
                        .getattr("passed")
                        .and_then(|v| v.extract::<bool>())
                        .unwrap_or(false);
                    let rationale = result
                        .getattr("rationale")
                        .and_then(|v| v.extract::<String>())
                        .unwrap_or_default();
                    let evidence = result
                        .getattr("evidence")
                        .and_then(|v| v.downcast::<PyDict>().map(|d| d.to_owned()).map_err(Into::into))
                        .ok()
                        .and_then(|d| {
                            let mut out = HashMap::new();
                            for (k, v) in d.iter() {
                                let k: String = k.extract().ok()?;
                                out.insert(k, py_to_json(v).ok()?);
                            }
                            Some(out)
                        })
                        .unwrap_or_default();
                    VerifierResult {
                        passed,
                        rationale,
                        evidence,
                    }
                }
                Err(e) => {
                    // Surface as panic so apply_verifier converts to
                    // VERIFIER_ERROR. We strip the Python err type
                    // name into the message.
                    let etype = e.get_type(py).name().unwrap_or("Exception").to_string();
                    let emsg = e.value(py).to_string();
                    panic!("{etype}: {emsg}");
                }
            }
        })
    }
}

#[pyfunction]
#[pyo3(signature = (verifier, workspace, trace, *, pipeline_done, iterations_run))]
pub fn apply_verifier(
    py: Python<'_>,
    verifier: Option<PyObject>,
    workspace: PyObject,
    trace: PyObject,
    pipeline_done: bool,
    iterations_run: u32,
) -> PyResult<PyTaskVerdict> {
    let workspace_path: PathBuf = workspace
        .as_ref(py)
        .str()?
        .to_str()?
        .to_string()
        .into();

    let verdict = if let Some(v) = verifier {
        let bridge = PyVerifier { inner: v };
        // Rust apply_verifier catches panics. The PyVerifier::verify
        // implementation panics on Python exception so the verdict
        // becomes VERIFIER_ERROR with the original message.
        let v = rs_apply(Some(&bridge), &workspace_path, &[], pipeline_done, iterations_run);
        if v.pattern == TaskPattern::VerifierError {
            // Strip the "PanicError: " prefix the Rust side prepends
            // so the message reads naturally on the Python side.
            let stripped = v
                .error
                .strip_prefix("PanicError: ")
                .unwrap_or(&v.error)
                .to_string();
            TaskVerdict {
                error: stripped,
                ..v
            }
        } else {
            v
        }
    } else {
        rs_apply::<PyVerifier>(None, &workspace_path, &[], pipeline_done, iterations_run)
    };

    let _ = trace; // accepted for API parity; not consumed in V1.0
    Ok(PyTaskVerdict { inner: verdict })
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyTaskPattern>()?;
    m.add_class::<PyVerifierResult>()?;
    m.add_class::<PyTaskVerdict>()?;
    m.add_function(wrap_pyfunction!(derive_task_pattern, m)?)?;
    m.add_function(wrap_pyfunction!(apply_verifier, m)?)?;
    Ok(())
}
