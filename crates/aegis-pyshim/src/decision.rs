//! PyO3 wrapper around `aegis_decision::DecisionPattern` +
//! `derive_pattern`.
//!
//! `derive_pattern` accepts either a `PyIterationEvent` (introduced
//! later when V1.3 ports the loop) or any duck-typed Python object
//! that exposes the IterationEvent attribute set used by the
//! deriver. The duck-typed path keeps the V0.x Python
//! `aegis.runtime.pipeline.IterationEvent` dataclass working
//! unchanged through V1.0–V1.2.

use aegis_decision::{derive_pattern as rs_derive, DecisionPattern, IterationEvent};
use pyo3::prelude::*;
use pyo3::types::PyType;

#[pyclass(name = "DecisionPattern", module = "aegis._core")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PyDecisionPattern {
    #[pyo3(name = "APPLIED_DONE")]
    AppliedDone,
    #[pyo3(name = "APPLIED_CONTINUING")]
    AppliedContinuing,
    #[pyo3(name = "REGRESSION_ROLLBACK")]
    RegressionRollback,
    #[pyo3(name = "EXECUTOR_FAILURE")]
    ExecutorFailure,
    #[pyo3(name = "SILENT_DONE_VETO")]
    SilentDoneVeto,
    #[pyo3(name = "VALIDATION_VETO")]
    ValidationVeto,
    #[pyo3(name = "NOOP_DONE")]
    NoopDone,
    #[pyo3(name = "STALEMATE_DETECTED")]
    StalemateDetected,
    #[pyo3(name = "THRASHING_DETECTED")]
    ThrashingDetected,
    #[pyo3(name = "UNKNOWN")]
    Unknown,
}

impl From<DecisionPattern> for PyDecisionPattern {
    fn from(p: DecisionPattern) -> Self {
        match p {
            DecisionPattern::AppliedDone => Self::AppliedDone,
            DecisionPattern::AppliedContinuing => Self::AppliedContinuing,
            DecisionPattern::RegressionRollback => Self::RegressionRollback,
            DecisionPattern::ExecutorFailure => Self::ExecutorFailure,
            DecisionPattern::SilentDoneVeto => Self::SilentDoneVeto,
            DecisionPattern::ValidationVeto => Self::ValidationVeto,
            DecisionPattern::NoopDone => Self::NoopDone,
            DecisionPattern::StalemateDetected => Self::StalemateDetected,
            DecisionPattern::ThrashingDetected => Self::ThrashingDetected,
            DecisionPattern::Unknown => Self::Unknown,
        }
    }
}

impl From<PyDecisionPattern> for DecisionPattern {
    fn from(p: PyDecisionPattern) -> Self {
        match p {
            PyDecisionPattern::AppliedDone => Self::AppliedDone,
            PyDecisionPattern::AppliedContinuing => Self::AppliedContinuing,
            PyDecisionPattern::RegressionRollback => Self::RegressionRollback,
            PyDecisionPattern::ExecutorFailure => Self::ExecutorFailure,
            PyDecisionPattern::SilentDoneVeto => Self::SilentDoneVeto,
            PyDecisionPattern::ValidationVeto => Self::ValidationVeto,
            PyDecisionPattern::NoopDone => Self::NoopDone,
            PyDecisionPattern::StalemateDetected => Self::StalemateDetected,
            PyDecisionPattern::ThrashingDetected => Self::ThrashingDetected,
            PyDecisionPattern::Unknown => Self::Unknown,
        }
    }
}

#[pymethods]
impl PyDecisionPattern {
    #[getter]
    fn value(&self) -> &'static str {
        DecisionPattern::from(*self).as_str()
    }

    fn __str__(&self) -> &'static str {
        self.value()
    }

    fn __repr__(&self) -> String {
        format!("DecisionPattern.{}", self.name())
    }

    fn __hash__(&self) -> isize {
        *self as isize
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyDecisionPattern>() {
            return *self == other;
        }
        // Allow string equality so `pattern == "applied_done"` works
        // (matches the Python str-Enum subclass behaviour).
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
            Self::AppliedDone => "APPLIED_DONE",
            Self::AppliedContinuing => "APPLIED_CONTINUING",
            Self::RegressionRollback => "REGRESSION_ROLLBACK",
            Self::ExecutorFailure => "EXECUTOR_FAILURE",
            Self::SilentDoneVeto => "SILENT_DONE_VETO",
            Self::ValidationVeto => "VALIDATION_VETO",
            Self::NoopDone => "NOOP_DONE",
            Self::StalemateDetected => "STALEMATE_DETECTED",
            Self::ThrashingDetected => "THRASHING_DETECTED",
            Self::Unknown => "UNKNOWN",
        }
    }

    #[classmethod]
    fn from_value(_cls: &PyType, s: &str) -> PyResult<Self> {
        DecisionPattern::from_str(s)
            .map(Self::from)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown pattern {s}")))
    }

    /// All variants in declaration order. PyO3 0.20 doesn't expose
    /// metaclass `__iter__` for `#[pyclass] enum`, so callers that
    /// want enumeration use this. Python's `aegis.runtime.decision_pattern`
    /// re-exports it as `DecisionPattern.members()`.
    #[classmethod]
    fn members(_cls: &PyType) -> Vec<Self> {
        vec![
            Self::AppliedDone,
            Self::AppliedContinuing,
            Self::RegressionRollback,
            Self::ExecutorFailure,
            Self::SilentDoneVeto,
            Self::ValidationVeto,
            Self::NoopDone,
            Self::StalemateDetected,
            Self::ThrashingDetected,
            Self::Unknown,
        ]
    }
}

fn read_attr<T: for<'a> FromPyObject<'a> + Default>(obj: &PyAny, name: &str) -> PyResult<T> {
    match obj.getattr(name) {
        Ok(v) => v.extract().or_else(|_| Ok(T::default())),
        Err(_) => Ok(T::default()),
    }
}

fn ev_from_py(obj: &PyAny) -> PyResult<IterationEvent> {
    Ok(IterationEvent {
        iteration: read_attr::<i64>(obj, "iteration")?.max(0) as u32,
        plan_id: read_attr::<String>(obj, "plan_id")?,
        plan_done: read_attr(obj, "plan_done")?,
        plan_patches: read_attr::<i64>(obj, "plan_patches")?.max(0) as u32,
        validation_passed: read_attr(obj, "validation_passed")?,
        applied: read_attr(obj, "applied")?,
        rolled_back: read_attr(obj, "rolled_back")?,
        regressed: read_attr(obj, "regressed")?,
        silent_done_contradiction: read_attr(obj, "silent_done_contradiction")?,
        stalemate_detected: read_attr(obj, "stalemate_detected")?,
        thrashing_detected: read_attr(obj, "thrashing_detected")?,
    })
}

#[pyfunction]
pub fn derive_pattern(ev: &PyAny) -> PyResult<PyDecisionPattern> {
    let ev = ev_from_py(ev)?;
    Ok(rs_derive(&ev).into())
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyDecisionPattern>()?;
    m.add_function(wrap_pyfunction!(derive_pattern, m)?)?;
    Ok(())
}
