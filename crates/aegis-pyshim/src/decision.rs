//! PyO3 wrapper around `aegis_decision::DecisionPattern` +
//! `derive_pattern`.
//!
//! `derive_pattern` accepts either a `PyIterationEvent` (introduced
//! later when V1.3 ports the loop) or any duck-typed Python object
//! that exposes the IterationEvent attribute set used by the
//! deriver. The duck-typed path keeps the V0.x Python
//! `aegis.runtime.pipeline.IterationEvent` dataclass working
//! unchanged through V1.0–V1.2.

use std::collections::BTreeMap;

use aegis_decision::{derive_pattern as rs_derive, DecisionPattern, IterationEvent};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

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
    // Direct path: native PyIterationEvent. Avoids the
    // duck-typed attribute walk + map round-trips on every iter.
    if let Ok(native) = obj.extract::<PyIterationEvent>() {
        return Ok(native.inner);
    }
    Ok(IterationEvent {
        iteration: read_attr::<i64>(obj, "iteration")?.max(0) as u32,
        plan_id: read_attr::<String>(obj, "plan_id")?,
        plan_goal: read_attr::<String>(obj, "plan_goal")?,
        plan_strategy: read_attr::<String>(obj, "plan_strategy")?,
        plan_done: read_attr(obj, "plan_done")?,
        plan_patches: read_attr::<i64>(obj, "plan_patches")?.max(0) as u32,
        validation_passed: read_attr(obj, "validation_passed")?,
        validation_errors: read_attr_list_str(obj, "validation_errors")?,
        applied: read_attr(obj, "applied")?,
        rolled_back: read_attr(obj, "rolled_back")?,
        regressed: read_attr(obj, "regressed")?,
        signals_total: read_attr::<i64>(obj, "signals_total")?.max(0) as u64,
        signals_by_kind: read_attr_dict_i64(obj, "signals_by_kind")?,
        signal_delta_vs_prev: read_attr_dict_i64(obj, "signal_delta_vs_prev")?,
        signal_value_totals: read_attr_dict_f64(obj, "signal_value_totals")?,
        signal_value_delta_vs_prev: read_attr_dict_f64(obj, "signal_value_delta_vs_prev")?,
        regression_detail: read_attr_dict_f64(obj, "regression_detail")?,
        stalemate_detected: read_attr(obj, "stalemate_detected")?,
        thrashing_detected: read_attr(obj, "thrashing_detected")?,
    })
}

fn read_attr_list_str(obj: &PyAny, name: &str) -> PyResult<Vec<String>> {
    let v = match obj.getattr(name) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    if v.is_none() {
        return Ok(Vec::new());
    }
    let l: &PyList = v.downcast()?;
    let mut out = Vec::with_capacity(l.len());
    for item in l.iter() {
        out.push(item.extract::<String>()?);
    }
    Ok(out)
}

fn read_attr_dict_i64(obj: &PyAny, name: &str) -> PyResult<BTreeMap<String, i64>> {
    let v = match obj.getattr(name) {
        Ok(v) => v,
        Err(_) => return Ok(BTreeMap::new()),
    };
    if v.is_none() {
        return Ok(BTreeMap::new());
    }
    let d: &PyDict = v.downcast()?;
    let mut out = BTreeMap::new();
    for (k, vv) in d.iter() {
        out.insert(k.extract::<String>()?, vv.extract::<i64>()?);
    }
    Ok(out)
}

fn read_attr_dict_f64(obj: &PyAny, name: &str) -> PyResult<BTreeMap<String, f64>> {
    let v = match obj.getattr(name) {
        Ok(v) => v,
        Err(_) => return Ok(BTreeMap::new()),
    };
    if v.is_none() {
        return Ok(BTreeMap::new());
    }
    let d: &PyDict = v.downcast()?;
    let mut out = BTreeMap::new();
    for (k, vv) in d.iter() {
        out.insert(k.extract::<String>()?, vv.extract::<f64>()?);
    }
    Ok(out)
}

// ---------- PyIterationEvent ----------

#[pyclass(name = "IterationEvent", module = "aegis._core")]
#[derive(Clone)]
pub struct PyIterationEvent {
    pub(crate) inner: IterationEvent,
}

impl PyIterationEvent {
    pub(crate) fn from_inner(inner: IterationEvent) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyIterationEvent {
    #[new]
    #[pyo3(signature = (
        iteration=0_i64,
        plan_id="".to_string(),
        plan_goal="".to_string(),
        plan_strategy="".to_string(),
        plan_done=false,
        plan_patches=0_i64,
        validation_passed=false,
        validation_errors=None,
        applied=false,
        rolled_back=false,
        regressed=false,
        signals_total=0_i64,
        signals_by_kind=None,
        signal_delta_vs_prev=None,
        signal_value_totals=None,
        signal_value_delta_vs_prev=None,
        regression_detail=None,
        stalemate_detected=false,
        thrashing_detected=false,
    ))]
    fn new(
        iteration: i64,
        plan_id: String,
        plan_goal: String,
        plan_strategy: String,
        plan_done: bool,
        plan_patches: i64,
        validation_passed: bool,
        validation_errors: Option<&PyList>,
        applied: bool,
        rolled_back: bool,
        regressed: bool,
        signals_total: i64,
        signals_by_kind: Option<&PyDict>,
        signal_delta_vs_prev: Option<&PyDict>,
        signal_value_totals: Option<&PyDict>,
        signal_value_delta_vs_prev: Option<&PyDict>,
        regression_detail: Option<&PyDict>,
        stalemate_detected: bool,
        thrashing_detected: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: IterationEvent {
                iteration: iteration.max(0) as u32,
                plan_id,
                plan_goal,
                plan_strategy,
                plan_done,
                plan_patches: plan_patches.max(0) as u32,
                validation_passed,
                validation_errors: list_to_string_vec(validation_errors)?,
                applied,
                rolled_back,
                regressed,
                signals_total: signals_total.max(0) as u64,
                signals_by_kind: dict_to_i64_map(signals_by_kind)?,
                signal_delta_vs_prev: dict_to_i64_map(signal_delta_vs_prev)?,
                signal_value_totals: dict_to_f64_map(signal_value_totals)?,
                signal_value_delta_vs_prev: dict_to_f64_map(signal_value_delta_vs_prev)?,
                regression_detail: dict_to_f64_map(regression_detail)?,
                stalemate_detected,
                thrashing_detected,
            },
        })
    }

    #[getter]
    fn iteration(&self) -> u32 {
        self.inner.iteration
    }
    #[getter]
    fn plan_id(&self) -> &str {
        &self.inner.plan_id
    }
    #[getter]
    fn plan_goal(&self) -> &str {
        &self.inner.plan_goal
    }
    #[getter]
    fn plan_strategy(&self) -> &str {
        &self.inner.plan_strategy
    }
    #[getter]
    fn plan_done(&self) -> bool {
        self.inner.plan_done
    }
    #[getter]
    fn plan_patches(&self) -> u32 {
        self.inner.plan_patches
    }
    #[getter]
    fn validation_passed(&self) -> bool {
        self.inner.validation_passed
    }
    #[getter]
    fn validation_errors(&self) -> Vec<String> {
        self.inner.validation_errors.clone()
    }
    #[getter]
    fn applied(&self) -> bool {
        self.inner.applied
    }
    #[getter]
    fn rolled_back(&self) -> bool {
        self.inner.rolled_back
    }
    #[getter]
    fn regressed(&self) -> bool {
        self.inner.regressed
    }
    #[getter]
    fn signals_total(&self) -> u64 {
        self.inner.signals_total
    }
    #[getter]
    fn signals_by_kind<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        i64_map_to_dict(py, &self.inner.signals_by_kind)
    }
    #[getter]
    fn signal_delta_vs_prev<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        i64_map_to_dict(py, &self.inner.signal_delta_vs_prev)
    }
    #[getter]
    fn signal_value_totals<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        f64_map_to_dict(py, &self.inner.signal_value_totals)
    }
    #[getter]
    fn signal_value_delta_vs_prev<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        f64_map_to_dict(py, &self.inner.signal_value_delta_vs_prev)
    }
    #[getter]
    fn regression_detail<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        f64_map_to_dict(py, &self.inner.regression_detail)
    }
    #[getter]
    fn stalemate_detected(&self) -> bool {
        self.inner.stalemate_detected
    }
    #[getter]
    fn thrashing_detected(&self) -> bool {
        self.inner.thrashing_detected
    }

    /// Computed @property: planner declared done but the patch never
    /// reached disk. Pipeline correctly ignored the flag, but a
    /// downstream observer wants to surface this loudly.
    #[getter]
    fn silent_done_contradiction(&self) -> bool {
        self.inner.silent_done_contradiction()
    }

    /// Computed @property: which named pattern this iteration's
    /// outcome falls into. Calls into `aegis_decision::derive_pattern`.
    #[getter]
    fn decision_pattern(&self) -> PyDecisionPattern {
        rs_derive(&self.inner).into()
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        let d = PyDict::new(py);
        d.set_item("iteration", self.inner.iteration)?;
        d.set_item("plan_id", &self.inner.plan_id)?;
        d.set_item("plan_goal", &self.inner.plan_goal)?;
        d.set_item("plan_strategy", &self.inner.plan_strategy)?;
        d.set_item("plan_done", self.inner.plan_done)?;
        d.set_item("plan_patches", self.inner.plan_patches)?;
        d.set_item("validation_passed", self.inner.validation_passed)?;
        d.set_item(
            "validation_errors",
            PyList::new(py, &self.inner.validation_errors),
        )?;
        d.set_item("applied", self.inner.applied)?;
        d.set_item("rolled_back", self.inner.rolled_back)?;
        d.set_item("regressed", self.inner.regressed)?;
        d.set_item("signals_total", self.inner.signals_total)?;
        d.set_item("signals_by_kind", i64_map_to_dict(py, &self.inner.signals_by_kind)?)?;
        d.set_item(
            "signal_delta_vs_prev",
            i64_map_to_dict(py, &self.inner.signal_delta_vs_prev)?,
        )?;
        d.set_item(
            "signal_value_totals",
            f64_map_to_dict(py, &self.inner.signal_value_totals)?,
        )?;
        d.set_item(
            "signal_value_delta_vs_prev",
            f64_map_to_dict(py, &self.inner.signal_value_delta_vs_prev)?,
        )?;
        d.set_item(
            "regression_detail",
            f64_map_to_dict(py, &self.inner.regression_detail)?,
        )?;
        d.set_item("stalemate_detected", self.inner.stalemate_detected)?;
        d.set_item("thrashing_detected", self.inner.thrashing_detected)?;
        d.set_item("silent_done_contradiction", self.inner.silent_done_contradiction())?;
        d.set_item("decision_pattern", rs_derive(&self.inner).as_str())?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "IterationEvent(iteration={}, plan_id={:?}, plan_done={}, applied={}, rolled_back={}, regressed={})",
            self.inner.iteration,
            self.inner.plan_id,
            self.inner.plan_done,
            self.inner.applied,
            self.inner.rolled_back,
            self.inner.regressed
        )
    }
}

fn list_to_string_vec(list: Option<&PyList>) -> PyResult<Vec<String>> {
    let mut out = Vec::new();
    if let Some(l) = list {
        for item in l.iter() {
            out.push(item.extract::<String>()?);
        }
    }
    Ok(out)
}

fn dict_to_i64_map(d: Option<&PyDict>) -> PyResult<BTreeMap<String, i64>> {
    let mut out = BTreeMap::new();
    if let Some(d) = d {
        for (k, v) in d.iter() {
            out.insert(k.extract::<String>()?, v.extract::<i64>()?);
        }
    }
    Ok(out)
}

fn dict_to_f64_map(d: Option<&PyDict>) -> PyResult<BTreeMap<String, f64>> {
    let mut out = BTreeMap::new();
    if let Some(d) = d {
        for (k, v) in d.iter() {
            out.insert(k.extract::<String>()?, v.extract::<f64>()?);
        }
    }
    Ok(out)
}

fn i64_map_to_dict<'py>(
    py: Python<'py>,
    m: &BTreeMap<String, i64>,
) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    for (k, v) in m {
        d.set_item(k, v)?;
    }
    Ok(d)
}

fn f64_map_to_dict<'py>(
    py: Python<'py>,
    m: &BTreeMap<String, f64>,
) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    for (k, v) in m {
        d.set_item(k, v)?;
    }
    Ok(d)
}

#[pyfunction]
pub fn derive_pattern(ev: &PyAny) -> PyResult<PyDecisionPattern> {
    let ev = ev_from_py(ev)?;
    Ok(rs_derive(&ev).into())
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyDecisionPattern>()?;
    m.add_class::<PyIterationEvent>()?;
    m.add_function(wrap_pyfunction!(derive_pattern, m)?)?;
    Ok(())
}
