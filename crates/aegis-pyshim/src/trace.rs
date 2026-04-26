//! PyO3 wrapper around `aegis_trace::DecisionTrace` + DecisionEvent.
//!
//! Mirrors the Python `aegis.runtime.trace` API surface. The four
//! verb constants are exposed as module-level attributes so existing
//! callers (`from aegis.runtime.trace import PASS`) work unchanged
//! after the re-export.

use std::collections::HashMap;

use aegis_trace::{
    DecisionEvent as RsEvent, DecisionTrace as RsTrace, BLOCK as VBLOCK, OBSERVE as VOBSERVE,
    PASS as VPASS, WARN as VWARN,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

#[pyclass(name = "DecisionEvent", module = "aegis._core")]
#[derive(Clone)]
pub struct PyDecisionEvent {
    inner: RsEvent,
}

impl PyDecisionEvent {
    pub fn from_inner(inner: RsEvent) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyDecisionEvent {
    #[new]
    #[pyo3(signature = (layer, decision, reason="".to_string(), signals=None, metadata=None, timestamp=None))]
    fn new(
        layer: String,
        decision: String,
        reason: String,
        signals: Option<&PyDict>,
        metadata: Option<&PyDict>,
        timestamp: Option<f64>,
    ) -> PyResult<Self> {
        let signals = py_signals_to_rs(signals)?;
        let metadata = py_metadata_to_rs(metadata)?;
        let mut ev = RsEvent::new(layer, decision, reason, signals, metadata);
        if let Some(t) = timestamp {
            ev.timestamp = t;
        }
        Ok(Self { inner: ev })
    }

    #[getter]
    fn layer(&self) -> &str {
        &self.inner.layer
    }

    #[getter]
    fn decision(&self) -> &str {
        &self.inner.decision
    }

    #[getter]
    fn reason(&self) -> &str {
        &self.inner.reason
    }

    #[getter]
    fn signals<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        rs_signals_to_py(py, &self.inner.signals)
    }

    #[getter]
    fn metadata<'py>(&self, py: Python<'py>) -> PyResult<&'py PyDict> {
        rs_metadata_to_py(py, &self.inner.metadata)
    }

    #[getter]
    fn timestamp(&self) -> f64 {
        self.inner.timestamp
    }

    fn __repr__(&self) -> String {
        format!(
            "DecisionEvent(layer={:?}, decision={:?}, reason={:?})",
            self.inner.layer, self.inner.decision, self.inner.reason
        )
    }
}

#[pyclass(name = "DecisionTrace", module = "aegis._core")]
#[derive(Clone)]
pub struct PyDecisionTrace {
    inner: RsTrace,
}

#[pymethods]
impl PyDecisionTrace {
    #[new]
    fn new() -> Self {
        Self {
            inner: RsTrace::new(),
        }
    }

    #[getter]
    fn events<'py>(&self, py: Python<'py>) -> PyResult<&'py PyList> {
        let items: Vec<Py<PyDecisionEvent>> = self
            .inner
            .events
            .iter()
            .map(|e| Py::new(py, PyDecisionEvent::from_inner(e.clone())))
            .collect::<PyResult<_>>()?;
        Ok(PyList::new(py, items))
    }

    #[pyo3(signature = (layer, decision, reason="".to_string(), signals=None, metadata=None))]
    fn emit(
        &mut self,
        layer: String,
        decision: String,
        reason: String,
        signals: Option<&PyDict>,
        metadata: Option<&PyDict>,
    ) -> PyResult<PyDecisionEvent> {
        let signals = py_signals_to_rs(signals)?;
        let metadata = py_metadata_to_rs(metadata)?;
        let ev = self
            .inner
            .emit(layer, decision, reason, Some(signals), Some(metadata));
        Ok(PyDecisionEvent::from_inner(ev))
    }

    fn by_layer(&self, layer: &str) -> Vec<PyDecisionEvent> {
        self.inner
            .by_layer(layer)
            .into_iter()
            .map(PyDecisionEvent::from_inner)
            .collect()
    }

    fn by_decision(&self, decision: &str) -> Vec<PyDecisionEvent> {
        self.inner
            .by_decision(decision)
            .into_iter()
            .map(PyDecisionEvent::from_inner)
            .collect()
    }

    fn has_block(&self) -> bool {
        self.inner.has_block()
    }

    fn reasons(&self) -> Vec<String> {
        self.inner.reasons()
    }

    fn to_list<'py>(&self, py: Python<'py>) -> PyResult<&'py PyList> {
        let items: Vec<&PyDict> = self
            .inner
            .events
            .iter()
            .map(|e| event_to_dict(py, e))
            .collect::<PyResult<_>>()?;
        Ok(PyList::new(py, items))
    }

    fn __len__(&self) -> usize {
        self.inner.events.len()
    }
}

fn event_to_dict<'py>(py: Python<'py>, e: &RsEvent) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    d.set_item("layer", &e.layer)?;
    d.set_item("decision", &e.decision)?;
    d.set_item("reason", &e.reason)?;
    d.set_item("signals", rs_signals_to_py(py, &e.signals)?)?;
    d.set_item("metadata", rs_metadata_to_py(py, &e.metadata)?)?;
    d.set_item("timestamp", e.timestamp)?;
    Ok(d)
}

fn py_signals_to_rs(d: Option<&PyDict>) -> PyResult<HashMap<String, f64>> {
    let mut out = HashMap::new();
    if let Some(d) = d {
        for (k, v) in d.iter() {
            let k: String = k.extract()?;
            let v: f64 = v.extract()?;
            out.insert(k, v);
        }
    }
    Ok(out)
}

fn py_metadata_to_rs(d: Option<&PyDict>) -> PyResult<HashMap<String, serde_json::Value>> {
    let mut out = HashMap::new();
    if let Some(d) = d {
        for (k, v) in d.iter() {
            let k: String = k.extract()?;
            out.insert(k, py_to_json(v)?);
        }
    }
    Ok(out)
}

fn rs_signals_to_py<'py>(py: Python<'py>, m: &HashMap<String, f64>) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    for (k, v) in m {
        d.set_item(k, *v)?;
    }
    Ok(d)
}

fn rs_metadata_to_py<'py>(
    py: Python<'py>,
    m: &HashMap<String, serde_json::Value>,
) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    for (k, v) in m {
        d.set_item(k, json_to_py(py, v)?)?;
    }
    Ok(d)
}

pub(crate) fn py_to_json(v: &PyAny) -> PyResult<serde_json::Value> {
    if v.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = v.extract::<bool>() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Ok(i) = v.extract::<i64>() {
        return Ok(serde_json::Value::from(i));
    }
    if let Ok(f) = v.extract::<f64>() {
        return Ok(serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if let Ok(s) = v.extract::<String>() {
        return Ok(serde_json::Value::String(s));
    }
    if let Ok(seq) = v.downcast::<pyo3::types::PyList>() {
        let mut out = Vec::with_capacity(seq.len());
        for item in seq.iter() {
            out.push(py_to_json(item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Ok(seq) = v.downcast::<pyo3::types::PyTuple>() {
        let mut out = Vec::with_capacity(seq.len());
        for item in seq.iter() {
            out.push(py_to_json(item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Ok(map) = v.downcast::<PyDict>() {
        let mut out = serde_json::Map::new();
        for (k, vv) in map.iter() {
            let k: String = k.extract()?;
            out.insert(k, py_to_json(vv)?);
        }
        return Ok(serde_json::Value::Object(out));
    }
    // Fallback — represent unknown types as their repr string so the
    // metadata round-trip is lossy but never crashes.
    let r: String = v.repr()?.extract()?;
    Ok(serde_json::Value::String(r))
}

pub(crate) fn json_to_py<'py>(py: Python<'py>, v: &serde_json::Value) -> PyResult<PyObject> {
    Ok(match v {
        serde_json::Value::Null => py.None(),
        serde_json::Value::Bool(b) => b.into_py(py),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py(py)
            } else if let Some(f) = n.as_f64() {
                f.into_py(py)
            } else {
                py.None()
            }
        }
        serde_json::Value::String(s) => s.into_py(py),
        serde_json::Value::Array(arr) => {
            let items: Vec<PyObject> = arr
                .iter()
                .map(|x| json_to_py(py, x))
                .collect::<PyResult<_>>()?;
            PyList::new(py, items).into_py(py)
        }
        serde_json::Value::Object(map) => {
            let d = PyDict::new(py);
            for (k, vv) in map {
                d.set_item(k, json_to_py(py, vv)?)?;
            }
            d.into_py(py)
        }
    })
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyDecisionEvent>()?;
    m.add_class::<PyDecisionTrace>()?;
    m.add("PASS", VPASS)?;
    m.add("BLOCK", VBLOCK)?;
    m.add("WARN", VWARN)?;
    m.add("OBSERVE", VOBSERVE)?;
    Ok(())
}
