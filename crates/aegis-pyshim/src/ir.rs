//! PyO3 wrappers for `aegis_ir` — `PatchKind`, `PatchStatus`, `Edit`,
//! `Patch`, `PatchPlan`, `EditResult` + the four engine functions.
//!
//! Rust owns the data model; Python `aegis.ir.patch` and
//! `aegis.shared.edit_engine` re-export from `aegis._core`. The two
//! str-Enums (`PatchKind`, `PatchStatus`) keep their lowercase string
//! values so `patch.kind == "modify"` and `status == "applied"` still
//! work — the Python str-Enum subclass behaviour is mirrored via
//! `__eq__`.

use aegis_ir::{
    apply_edit as rs_apply_edit, apply_edits as rs_apply_edits, is_ok as rs_is_ok, Edit,
    EditResult, Patch, PatchKind, PatchPlan, PatchStatus,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

// ---------- enums ----------

#[pyclass(name = "PatchKind", module = "aegis._core")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PyPatchKind {
    #[pyo3(name = "CREATE")]
    Create,
    #[pyo3(name = "MODIFY")]
    Modify,
    #[pyo3(name = "DELETE")]
    Delete,
}

impl From<PatchKind> for PyPatchKind {
    fn from(k: PatchKind) -> Self {
        match k {
            PatchKind::Create => Self::Create,
            PatchKind::Modify => Self::Modify,
            PatchKind::Delete => Self::Delete,
        }
    }
}

impl From<PyPatchKind> for PatchKind {
    fn from(k: PyPatchKind) -> Self {
        match k {
            PyPatchKind::Create => Self::Create,
            PyPatchKind::Modify => Self::Modify,
            PyPatchKind::Delete => Self::Delete,
        }
    }
}

#[pymethods]
impl PyPatchKind {
    #[getter]
    fn value(&self) -> &'static str {
        PatchKind::from(*self).as_str()
    }

    #[getter]
    fn name(&self) -> &'static str {
        match self {
            Self::Create => "CREATE",
            Self::Modify => "MODIFY",
            Self::Delete => "DELETE",
        }
    }

    fn __str__(&self) -> &'static str {
        self.value()
    }

    fn __repr__(&self) -> String {
        format!("PatchKind.{}", self.name())
    }

    fn __hash__(&self) -> isize {
        *self as isize
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyPatchKind>() {
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

    #[classmethod]
    fn from_value(_cls: &PyType, s: &str) -> PyResult<Self> {
        PatchKind::from_str(s)
            .map(Self::from)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown PatchKind {s}")))
    }

    #[classmethod]
    fn members(_cls: &PyType) -> Vec<Self> {
        vec![Self::Create, Self::Modify, Self::Delete]
    }
}

#[pyclass(name = "PatchStatus", module = "aegis._core")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PyPatchStatus {
    #[pyo3(name = "APPLIED")]
    Applied,
    #[pyo3(name = "ALREADY_APPLIED")]
    AlreadyApplied,
    #[pyo3(name = "NOT_FOUND")]
    NotFound,
    #[pyo3(name = "AMBIGUOUS")]
    Ambiguous,
}

impl From<PatchStatus> for PyPatchStatus {
    fn from(s: PatchStatus) -> Self {
        match s {
            PatchStatus::Applied => Self::Applied,
            PatchStatus::AlreadyApplied => Self::AlreadyApplied,
            PatchStatus::NotFound => Self::NotFound,
            PatchStatus::Ambiguous => Self::Ambiguous,
        }
    }
}

impl From<PyPatchStatus> for PatchStatus {
    fn from(s: PyPatchStatus) -> Self {
        match s {
            PyPatchStatus::Applied => Self::Applied,
            PyPatchStatus::AlreadyApplied => Self::AlreadyApplied,
            PyPatchStatus::NotFound => Self::NotFound,
            PyPatchStatus::Ambiguous => Self::Ambiguous,
        }
    }
}

#[pymethods]
impl PyPatchStatus {
    #[getter]
    fn value(&self) -> &'static str {
        PatchStatus::from(*self).as_str()
    }

    #[getter]
    fn name(&self) -> &'static str {
        match self {
            Self::Applied => "APPLIED",
            Self::AlreadyApplied => "ALREADY_APPLIED",
            Self::NotFound => "NOT_FOUND",
            Self::Ambiguous => "AMBIGUOUS",
        }
    }

    fn __str__(&self) -> &'static str {
        self.value()
    }

    fn __repr__(&self) -> String {
        format!("PatchStatus.{}", self.name())
    }

    fn __hash__(&self) -> isize {
        *self as isize
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyPatchStatus>() {
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

    #[classmethod]
    fn from_value(_cls: &PyType, s: &str) -> PyResult<Self> {
        PatchStatus::from_str(s)
            .map(Self::from)
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("unknown PatchStatus {s}"))
            })
    }

    #[classmethod]
    fn members(_cls: &PyType) -> Vec<Self> {
        vec![
            Self::Applied,
            Self::AlreadyApplied,
            Self::NotFound,
            Self::Ambiguous,
        ]
    }
}

// ---------- data classes ----------

#[pyclass(name = "Edit", module = "aegis._core")]
#[derive(Clone)]
pub struct PyEdit {
    inner: Edit,
}

impl PyEdit {
    fn from_inner(inner: Edit) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyEdit {
    #[new]
    #[pyo3(signature = (
        old_string="".to_string(),
        new_string="".to_string(),
        context_before="".to_string(),
        context_after="".to_string()
    ))]
    fn new(
        old_string: String,
        new_string: String,
        context_before: String,
        context_after: String,
    ) -> Self {
        Self {
            inner: Edit {
                old_string,
                new_string,
                context_before,
                context_after,
            },
        }
    }

    #[getter]
    fn old_string(&self) -> &str {
        &self.inner.old_string
    }
    #[setter]
    fn set_old_string(&mut self, v: String) {
        self.inner.old_string = v;
    }

    #[getter]
    fn new_string(&self) -> &str {
        &self.inner.new_string
    }
    #[setter]
    fn set_new_string(&mut self, v: String) {
        self.inner.new_string = v;
    }

    #[getter]
    fn context_before(&self) -> &str {
        &self.inner.context_before
    }
    #[setter]
    fn set_context_before(&mut self, v: String) {
        self.inner.context_before = v;
    }

    #[getter]
    fn context_after(&self) -> &str {
        &self.inner.context_after
    }
    #[setter]
    fn set_context_after(&mut self, v: String) {
        self.inner.context_after = v;
    }

    fn __repr__(&self) -> String {
        format!(
            "Edit(old_string={:?}, new_string={:?}, context_before={:?}, context_after={:?})",
            self.inner.old_string,
            self.inner.new_string,
            self.inner.context_before,
            self.inner.context_after
        )
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyEdit>() {
            return self.inner == other.inner;
        }
        false
    }
}

#[pyclass(name = "Patch", module = "aegis._core")]
#[derive(Clone)]
pub struct PyPatch {
    inner: Patch,
}

impl PyPatch {
    fn from_inner(inner: Patch) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyPatch {
    #[new]
    #[pyo3(signature = (
        id,
        kind,
        path,
        rationale="".to_string(),
        content=None,
        edits=None
    ))]
    fn new(
        id: String,
        kind: &PyAny,
        path: String,
        rationale: String,
        content: Option<String>,
        edits: Option<&PyList>,
    ) -> PyResult<Self> {
        let kind = extract_kind(kind)?;
        let edits = extract_edits(edits)?;
        Ok(Self {
            inner: Patch {
                id,
                kind,
                path,
                rationale,
                content,
                edits,
            },
        })
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }
    #[setter]
    fn set_id(&mut self, v: String) {
        self.inner.id = v;
    }

    #[getter]
    fn kind(&self) -> PyPatchKind {
        self.inner.kind.into()
    }
    #[setter]
    fn set_kind(&mut self, v: &PyAny) -> PyResult<()> {
        self.inner.kind = extract_kind(v)?;
        Ok(())
    }

    #[getter]
    fn path(&self) -> &str {
        &self.inner.path
    }
    #[setter]
    fn set_path(&mut self, v: String) {
        self.inner.path = v;
    }

    #[getter]
    fn rationale(&self) -> &str {
        &self.inner.rationale
    }
    #[setter]
    fn set_rationale(&mut self, v: String) {
        self.inner.rationale = v;
    }

    #[getter]
    fn content(&self) -> Option<&str> {
        self.inner.content.as_deref()
    }
    #[setter]
    fn set_content(&mut self, v: Option<String>) {
        self.inner.content = v;
    }

    #[getter]
    fn edits<'py>(&self, py: Python<'py>) -> PyResult<&'py PyList> {
        let items: Vec<Py<PyEdit>> = self
            .inner
            .edits
            .iter()
            .map(|e| Py::new(py, PyEdit::from_inner(e.clone())))
            .collect::<PyResult<_>>()?;
        Ok(PyList::new(py, items))
    }
    #[setter]
    fn set_edits(&mut self, v: &PyList) -> PyResult<()> {
        self.inner.edits = extract_edits(Some(v))?;
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!(
            "Patch(id={:?}, kind=PatchKind.{}, path={:?}, edits={} items)",
            self.inner.id,
            PyPatchKind::from(self.inner.kind).name(),
            self.inner.path,
            self.inner.edits.len()
        )
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyPatch>() {
            return self.inner == other.inner;
        }
        false
    }
}

#[pyclass(name = "PatchPlan", module = "aegis._core")]
#[derive(Clone)]
pub struct PyPatchPlan {
    inner: PatchPlan,
}

impl PyPatchPlan {
    fn from_inner(inner: PatchPlan) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyPatchPlan {
    #[new]
    #[pyo3(signature = (
        goal,
        strategy,
        patches=None,
        target_files=None,
        done=false,
        iteration=0_i64,
        parent_id=None
    ))]
    fn new(
        goal: String,
        strategy: String,
        patches: Option<&PyList>,
        target_files: Option<&PyList>,
        done: bool,
        iteration: i64,
        parent_id: Option<String>,
    ) -> PyResult<Self> {
        let patches = extract_patches(patches)?;
        let target_files = extract_target_files(target_files)?;
        Ok(Self {
            inner: PatchPlan {
                goal,
                strategy,
                patches,
                target_files,
                done,
                iteration: iteration.max(0) as u32,
                parent_id,
            },
        })
    }

    #[getter]
    fn goal(&self) -> &str {
        &self.inner.goal
    }
    #[setter]
    fn set_goal(&mut self, v: String) {
        self.inner.goal = v;
    }

    #[getter]
    fn strategy(&self) -> &str {
        &self.inner.strategy
    }
    #[setter]
    fn set_strategy(&mut self, v: String) {
        self.inner.strategy = v;
    }

    #[getter]
    fn patches<'py>(&self, py: Python<'py>) -> PyResult<&'py PyList> {
        let items: Vec<Py<PyPatch>> = self
            .inner
            .patches
            .iter()
            .map(|p| Py::new(py, PyPatch::from_inner(p.clone())))
            .collect::<PyResult<_>>()?;
        Ok(PyList::new(py, items))
    }
    #[setter]
    fn set_patches(&mut self, v: &PyList) -> PyResult<()> {
        self.inner.patches = extract_patches(Some(v))?;
        Ok(())
    }

    #[getter]
    fn target_files(&self) -> Vec<String> {
        self.inner.target_files.clone()
    }
    #[setter]
    fn set_target_files(&mut self, v: &PyList) -> PyResult<()> {
        self.inner.target_files = extract_target_files(Some(v))?;
        Ok(())
    }

    #[getter]
    fn done(&self) -> bool {
        self.inner.done
    }
    #[setter]
    fn set_done(&mut self, v: bool) {
        self.inner.done = v;
    }

    #[getter]
    fn iteration(&self) -> u32 {
        self.inner.iteration
    }
    #[setter]
    fn set_iteration(&mut self, v: i64) {
        self.inner.iteration = v.max(0) as u32;
    }

    #[getter]
    fn parent_id(&self) -> Option<String> {
        self.inner.parent_id.clone()
    }
    #[setter]
    fn set_parent_id(&mut self, v: Option<String>) {
        self.inner.parent_id = v;
    }

    fn __repr__(&self) -> String {
        format!(
            "PatchPlan(goal={:?}, strategy={:?}, patches={} items, iteration={}, done={})",
            self.inner.goal,
            self.inner.strategy,
            self.inner.patches.len(),
            self.inner.iteration,
            self.inner.done
        )
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyPatchPlan>() {
            return self.inner == other.inner;
        }
        false
    }
}

#[pyclass(name = "EditResult", module = "aegis._core")]
#[derive(Clone)]
pub struct PyEditResult {
    inner: EditResult,
}

#[pymethods]
impl PyEditResult {
    #[new]
    #[pyo3(signature = (status, matches=0))]
    fn new(status: &PyAny, matches: usize) -> PyResult<Self> {
        let status = extract_status(status)?;
        Ok(Self {
            inner: EditResult { status, matches },
        })
    }

    #[getter]
    fn status(&self) -> PyPatchStatus {
        self.inner.status.into()
    }

    #[getter]
    fn matches(&self) -> usize {
        self.inner.matches
    }

    fn __repr__(&self) -> String {
        format!(
            "EditResult(status=PatchStatus.{}, matches={})",
            PyPatchStatus::from(self.inner.status).name(),
            self.inner.matches
        )
    }

    fn __eq__(&self, other: &PyAny) -> bool {
        if let Ok(other) = other.extract::<PyEditResult>() {
            return self.inner == other.inner;
        }
        false
    }
}

// ---------- engine functions ----------

#[pyfunction]
pub fn apply_edit(content: &str, edit: &PyEdit) -> (String, PyEditResult) {
    let (new_content, result) = rs_apply_edit(content, &edit.inner);
    (new_content, PyEditResult { inner: result })
}

#[pyfunction]
pub fn apply_edits(content: &str, edits: &PyList) -> PyResult<(String, Vec<PyEditResult>)> {
    let edits = extract_edits(Some(edits))?;
    let (new_content, results) = rs_apply_edits(content, &edits);
    let py_results = results
        .into_iter()
        .map(|r| PyEditResult { inner: r })
        .collect();
    Ok((new_content, py_results))
}

#[pyfunction]
pub fn is_ok(status: &PyAny) -> PyResult<bool> {
    let s = extract_status(status)?;
    Ok(rs_is_ok(s))
}

#[pyfunction]
pub fn patch_to_dict<'py>(py: Python<'py>, patch: &PyPatch) -> PyResult<&'py PyDict> {
    patch_to_pydict(py, &patch.inner)
}

#[pyfunction]
pub fn patch_from_dict(data: &PyDict) -> PyResult<PyPatch> {
    let p = patch_from_pydict(data)?;
    Ok(PyPatch { inner: p })
}

#[pyfunction]
pub fn plan_to_dict<'py>(py: Python<'py>, plan: &PyPatchPlan) -> PyResult<&'py PyDict> {
    plan_to_pydict(py, &plan.inner)
}

#[pyfunction]
pub fn plan_from_dict(data: &PyDict) -> PyResult<PyPatchPlan> {
    let p = plan_from_pydict(data)?;
    Ok(PyPatchPlan { inner: p })
}

// ---------- helpers ----------

fn extract_kind(v: &PyAny) -> PyResult<PatchKind> {
    if let Ok(k) = v.extract::<PyPatchKind>() {
        return Ok(k.into());
    }
    if let Ok(s) = v.extract::<String>() {
        return PatchKind::from_str(&s).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("unknown PatchKind {s}"))
        });
    }
    Err(pyo3::exceptions::PyTypeError::new_err(
        "kind must be PatchKind or str",
    ))
}

fn extract_status(v: &PyAny) -> PyResult<PatchStatus> {
    if let Ok(s) = v.extract::<PyPatchStatus>() {
        return Ok(s.into());
    }
    if let Ok(s) = v.extract::<String>() {
        return PatchStatus::from_str(&s).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("unknown PatchStatus {s}"))
        });
    }
    Err(pyo3::exceptions::PyTypeError::new_err(
        "status must be PatchStatus or str",
    ))
}

fn extract_edits(items: Option<&PyList>) -> PyResult<Vec<Edit>> {
    let mut out = Vec::new();
    if let Some(items) = items {
        for item in items.iter() {
            if let Ok(e) = item.extract::<PyEdit>() {
                out.push(e.inner);
                continue;
            }
            if let Ok(d) = item.downcast::<PyDict>() {
                out.push(edit_from_pydict(d)?);
                continue;
            }
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "edits must contain Edit instances or dicts",
            ));
        }
    }
    Ok(out)
}

fn extract_patches(items: Option<&PyList>) -> PyResult<Vec<Patch>> {
    let mut out = Vec::new();
    if let Some(items) = items {
        for item in items.iter() {
            if let Ok(p) = item.extract::<PyPatch>() {
                out.push(p.inner);
                continue;
            }
            if let Ok(d) = item.downcast::<PyDict>() {
                out.push(patch_from_pydict(d)?);
                continue;
            }
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "patches must contain Patch instances or dicts",
            ));
        }
    }
    Ok(out)
}

fn extract_target_files(items: Option<&PyList>) -> PyResult<Vec<String>> {
    let mut out = Vec::new();
    if let Some(items) = items {
        for item in items.iter() {
            out.push(item.extract::<String>()?);
        }
    }
    Ok(out)
}

fn edit_to_pydict<'py>(py: Python<'py>, e: &Edit) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    d.set_item("old_string", &e.old_string)?;
    d.set_item("new_string", &e.new_string)?;
    d.set_item("context_before", &e.context_before)?;
    d.set_item("context_after", &e.context_after)?;
    Ok(d)
}

fn edit_from_pydict(d: &PyDict) -> PyResult<Edit> {
    Ok(Edit {
        old_string: get_str(d, "old_string", "")?,
        new_string: get_str(d, "new_string", "")?,
        context_before: get_str(d, "context_before", "")?,
        context_after: get_str(d, "context_after", "")?,
    })
}

fn patch_to_pydict<'py>(py: Python<'py>, p: &Patch) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    d.set_item("id", &p.id)?;
    d.set_item("kind", p.kind.as_str())?;
    d.set_item("path", &p.path)?;
    d.set_item("rationale", &p.rationale)?;
    d.set_item("content", p.content.as_deref())?;
    let edits: Vec<&PyDict> = p
        .edits
        .iter()
        .map(|e| edit_to_pydict(py, e))
        .collect::<PyResult<_>>()?;
    d.set_item("edits", PyList::new(py, edits))?;
    Ok(d)
}

fn patch_from_pydict(d: &PyDict) -> PyResult<Patch> {
    let kind_str = get_str(d, "kind", "")?;
    let kind = PatchKind::from_str(&kind_str).ok_or_else(|| {
        pyo3::exceptions::PyValueError::new_err(format!("unknown PatchKind {kind_str}"))
    })?;
    let edits = match d.get_item("edits")? {
        Some(v) => {
            let l: &PyList = v.downcast()?;
            let mut out = Vec::with_capacity(l.len());
            for item in l.iter() {
                let dd: &PyDict = item.downcast()?;
                out.push(edit_from_pydict(dd)?);
            }
            out
        }
        None => Vec::new(),
    };
    Ok(Patch {
        id: get_str(d, "id", "")?,
        kind,
        path: get_str(d, "path", "")?,
        rationale: get_str(d, "rationale", "")?,
        content: get_opt_str(d, "content")?,
        edits,
    })
}

fn plan_to_pydict<'py>(py: Python<'py>, p: &PatchPlan) -> PyResult<&'py PyDict> {
    let d = PyDict::new(py);
    d.set_item("goal", &p.goal)?;
    d.set_item("strategy", &p.strategy)?;
    let patches: Vec<&PyDict> = p
        .patches
        .iter()
        .map(|x| patch_to_pydict(py, x))
        .collect::<PyResult<_>>()?;
    d.set_item("patches", PyList::new(py, patches))?;
    d.set_item("target_files", PyList::new(py, &p.target_files))?;
    d.set_item("done", p.done)?;
    d.set_item("iteration", p.iteration)?;
    d.set_item("parent_id", p.parent_id.as_deref())?;
    Ok(d)
}

fn plan_from_pydict(d: &PyDict) -> PyResult<PatchPlan> {
    let patches = match d.get_item("patches")? {
        Some(v) => {
            let l: &PyList = v.downcast()?;
            let mut out = Vec::with_capacity(l.len());
            for item in l.iter() {
                let dd: &PyDict = item.downcast()?;
                out.push(patch_from_pydict(dd)?);
            }
            out
        }
        None => Vec::new(),
    };
    let target_files = match d.get_item("target_files")? {
        Some(v) => {
            let l: &PyList = v.downcast()?;
            let mut out = Vec::with_capacity(l.len());
            for item in l.iter() {
                out.push(item.extract::<String>()?);
            }
            out
        }
        None => Vec::new(),
    };
    Ok(PatchPlan {
        goal: get_str(d, "goal", "")?,
        strategy: get_str(d, "strategy", "")?,
        patches,
        target_files,
        done: get_bool(d, "done", false)?,
        iteration: get_u32(d, "iteration", 0)?,
        parent_id: get_opt_str(d, "parent_id")?,
    })
}

fn get_str(d: &PyDict, key: &str, default: &str) -> PyResult<String> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => v.extract(),
        _ => Ok(default.to_string()),
    }
}

fn get_opt_str(d: &PyDict, key: &str) -> PyResult<Option<String>> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(Some(v.extract()?)),
        _ => Ok(None),
    }
}

fn get_bool(d: &PyDict, key: &str, default: bool) -> PyResult<bool> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => v.extract(),
        _ => Ok(default),
    }
}

fn get_u32(d: &PyDict, key: &str, default: u32) -> PyResult<u32> {
    match d.get_item(key)? {
        Some(v) if !v.is_none() => Ok(v.extract::<i64>()?.max(0) as u32),
        _ => Ok(default),
    }
}

// ---------- module registration ----------

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyPatchKind>()?;
    m.add_class::<PyPatchStatus>()?;
    m.add_class::<PyEdit>()?;
    m.add_class::<PyPatch>()?;
    m.add_class::<PyPatchPlan>()?;
    m.add_class::<PyEditResult>()?;
    m.add_function(wrap_pyfunction!(apply_edit, m)?)?;
    m.add_function(wrap_pyfunction!(apply_edits, m)?)?;
    m.add_function(wrap_pyfunction!(is_ok, m)?)?;
    m.add_function(wrap_pyfunction!(patch_to_dict, m)?)?;
    m.add_function(wrap_pyfunction!(patch_from_dict, m)?)?;
    m.add_function(wrap_pyfunction!(plan_to_dict, m)?)?;
    m.add_function(wrap_pyfunction!(plan_from_dict, m)?)?;
    Ok(())
}
