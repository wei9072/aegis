//! Pure-Rust implementation of `aegis._build_context` + `SignalLayer`.
//!
//! V1.9 — moves the context-building logic out of Python so the
//! eventual `aegis-cli` binary doesn't need a separate
//! `_build_context` callback. Today it's exposed as a PyO3 callable
//! (`aegis._core.build_context`); Python `aegis/runtime/pipeline.py`
//! uses it as the `ctx_builder` argument to `run_loop`.
//!
//! The function returns a `PyPlanContext` PyO3 class — a property
//! bag matching the V0.x `aegis.agents.planner.PlanContext` dataclass
//! so the `LLMPlanner.plan(ctx)` Python prompt-template code keeps
//! reading the same attributes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aegis_core::ast::registry::LanguageRegistry;
use aegis_core::ast::parser::get_imports_native;
use aegis_core::graph::cycle::has_cycle as graph_has_cycle;
use aegis_core::graph::dependency::DependencyGraph as RsDepGraph;
use aegis_core::signal_layer_pyapi::{extract_signals_native, Signal as PySignal, SignalData};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// V0.x `PlanContext` dataclass mirror. Property bag with mutable
/// `previous_*` slots the loop sets per iteration; the Planner
/// reads every field below.
#[pyclass(name = "PlanContext", module = "aegis._core")]
pub struct PyPlanContext {
    task: String,
    root: String,
    scope: Option<Vec<String>>,
    py_files: Vec<String>,
    /// signals[path] = list[Signal]. Stored as PyObject so the
    /// Python side can pass it to `aegis._core.kind_counts(...)`
    /// without a double-conversion. Built by `build_context`.
    signals: PyObject,
    graph_edges: Vec<(String, String)>,
    has_cycle: bool,
    file_snippets: HashMap<String, String>,
    previous_plan: Option<PyObject>,
    previous_errors: Vec<PyObject>,
    previous_result: Option<PyObject>,
    previous_regressed: bool,
    previous_regression_detail: HashMap<String, f64>,
}

#[pymethods]
impl PyPlanContext {
    #[new]
    #[pyo3(signature = (
        task,
        root,
        scope=None,
        py_files=None,
        signals=None,
        graph_edges=None,
        has_cycle=false,
        file_snippets=None,
    ))]
    fn new(
        py: Python<'_>,
        task: String,
        root: String,
        scope: Option<Vec<String>>,
        py_files: Option<Vec<String>>,
        signals: Option<PyObject>,
        graph_edges: Option<Vec<(String, String)>>,
        has_cycle: bool,
        file_snippets: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            task,
            root,
            scope,
            py_files: py_files.unwrap_or_default(),
            signals: signals.unwrap_or_else(|| PyDict::new(py).into()),
            graph_edges: graph_edges.unwrap_or_default(),
            has_cycle,
            file_snippets: file_snippets.unwrap_or_default(),
            previous_plan: None,
            previous_errors: Vec::new(),
            previous_result: None,
            previous_regressed: false,
            previous_regression_detail: HashMap::new(),
        }
    }

    #[getter]
    fn task(&self) -> &str {
        &self.task
    }
    #[getter]
    fn root(&self) -> &str {
        &self.root
    }
    #[getter]
    fn scope(&self, py: Python<'_>) -> PyObject {
        match &self.scope {
            Some(s) => PyList::new(py, s).into(),
            None => py.None(),
        }
    }
    #[getter]
    fn py_files(&self) -> Vec<String> {
        self.py_files.clone()
    }
    #[getter]
    fn signals(&self, py: Python<'_>) -> PyObject {
        self.signals.clone_ref(py)
    }
    #[getter]
    fn graph_edges(&self) -> Vec<(String, String)> {
        self.graph_edges.clone()
    }
    #[getter]
    fn has_cycle(&self) -> bool {
        self.has_cycle
    }
    #[getter]
    fn file_snippets(&self) -> HashMap<String, String> {
        self.file_snippets.clone()
    }

    #[getter]
    fn previous_plan(&self, py: Python<'_>) -> PyObject {
        self.previous_plan
            .as_ref()
            .map(|p| p.clone_ref(py))
            .unwrap_or_else(|| py.None())
    }
    #[setter]
    fn set_previous_plan(&mut self, v: Option<PyObject>) {
        self.previous_plan = v;
    }

    #[getter]
    fn previous_errors(&self, py: Python<'_>) -> Vec<PyObject> {
        self.previous_errors.iter().map(|p| p.clone_ref(py)).collect()
    }
    #[setter]
    fn set_previous_errors(&mut self, v: &PyList) -> PyResult<()> {
        let mut out = Vec::with_capacity(v.len());
        for item in v.iter() {
            out.push(item.into());
        }
        self.previous_errors = out;
        Ok(())
    }

    #[getter]
    fn previous_result(&self, py: Python<'_>) -> PyObject {
        self.previous_result
            .as_ref()
            .map(|p| p.clone_ref(py))
            .unwrap_or_else(|| py.None())
    }
    #[setter]
    fn set_previous_result(&mut self, v: Option<PyObject>) {
        self.previous_result = v;
    }

    #[getter]
    fn previous_regressed(&self) -> bool {
        self.previous_regressed
    }
    #[setter]
    fn set_previous_regressed(&mut self, v: bool) {
        self.previous_regressed = v;
    }

    #[getter]
    fn previous_regression_detail(&self) -> HashMap<String, f64> {
        self.previous_regression_detail.clone()
    }
    #[setter]
    fn set_previous_regression_detail(&mut self, v: HashMap<String, f64>) {
        self.previous_regression_detail = v;
    }

    fn __repr__(&self) -> String {
        format!(
            "PlanContext(task={:?}, root={:?}, py_files={} files, signals_files={}, has_cycle={})",
            self.task,
            self.root,
            self.py_files.len(),
            // Can't peek at PyDict without GIL; report file-count from py_files.
            self.py_files.len(),
            self.has_cycle
        )
    }
}

// ---------- pure-Rust context building ----------

/// Walk the workspace, extract signals + imports + cycles, return
/// a populated PyPlanContext. Mirrors `aegis/runtime/pipeline.py::_build_context`
/// one-for-one.
#[pyfunction]
#[pyo3(signature = (task, root, scope=None, include_snippets=true))]
pub fn build_context(
    py: Python<'_>,
    task: String,
    root: String,
    scope: Option<Vec<String>>,
    include_snippets: bool,
) -> PyResult<Py<PyPlanContext>> {
    let root_path =
        std::fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));

    let py_files_abs = discover_source_files(&root_path);
    let py_files_rel: Vec<String> = py_files_abs
        .iter()
        .filter_map(|p| {
            p.strip_prefix(&root_path)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        })
        .collect();

    // Per-file signals extraction. Signals get wrapped in PyO3
    // Signal class so the Python LLMPlanner reads .name / .value /
    // .description exactly like before.
    let signals_dict = PyDict::new(py);
    for (abs, rel) in py_files_abs.iter().zip(py_files_rel.iter()) {
        let abs_str = abs.to_string_lossy();
        let sig_data = match extract_signals_native(&abs_str) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if sig_data.is_empty() {
            continue;
        }
        let py_sigs: Vec<Py<PySignal>> = sig_data
            .into_iter()
            .map(|d: SignalData| {
                Py::new(
                    py,
                    PySignal::new(d.name, d.value, d.file_path, d.description),
                )
            })
            .collect::<PyResult<_>>()?;
        signals_dict.set_item(rel, PyList::new(py, py_sigs))?;
    }

    // Import graph + cycle detection. Module map mirrors
    // `aegis.ir.normalizer.build_module_map`.
    let module_map = build_module_map(&root_path, &py_files_abs);
    let mut edges: Vec<(String, String)> = Vec::new();
    for abs in &py_files_abs {
        let abs_str = abs.to_string_lossy();
        let imports = match get_imports_native(&abs_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for imp in imports {
            if let Some(target) = module_map.get(&imp) {
                edges.push((abs_str.to_string(), target.clone()));
            }
        }
    }
    let has_cycle = if edges.is_empty() {
        false
    } else {
        let mut graph = RsDepGraph::new();
        graph.build_from_edges(edges.clone());
        graph_has_cycle(&graph)
    };

    // File snippets — read up to 30 in-scope files for the prompt
    // (mirrors the V0.x cap so prompt size stays bounded).
    let mut snippets: HashMap<String, String> = HashMap::new();
    if include_snippets {
        let in_scope = scope_filter(&py_files_abs, &root_path, scope.as_deref());
        for abs in in_scope.iter().take(30) {
            if let Ok(rel) = abs.strip_prefix(&root_path) {
                if let Ok(body) = fs::read_to_string(abs) {
                    snippets.insert(rel.to_string_lossy().into_owned(), body);
                }
            }
        }
    }

    Py::new(
        py,
        PyPlanContext {
            task,
            root: root_path.to_string_lossy().into_owned(),
            scope,
            py_files: py_files_rel,
            signals: signals_dict.into(),
            graph_edges: edges,
            has_cycle,
            file_snippets: snippets,
            previous_plan: None,
            previous_errors: Vec::new(),
            previous_result: None,
            previous_regressed: false,
            previous_regression_detail: HashMap::new(),
        },
    )
}

// ---------- helpers ----------

fn discover_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let allowed_exts: Vec<&'static str> =
        LanguageRegistry::global().extensions().to_vec();
    walk(root, &allowed_exts, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, allowed_exts: &[&str], out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Skip hidden / pycache / venv-ish directories — same set the
        // Python `_discover_py_files` skipped.
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || name == "__pycache__" {
                continue;
            }
        }
        if path.is_dir() {
            walk(&path, allowed_exts, out);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let dotted = format!(".{}", ext);
                if allowed_exts.contains(&dotted.as_str()) {
                    out.push(path);
                }
            }
        }
    }
}

fn scope_filter(
    files: &[PathBuf],
    root: &Path,
    scope: Option<&[String]>,
) -> Vec<PathBuf> {
    let scope = match scope {
        Some(s) if !s.is_empty() => s,
        _ => return files.to_vec(),
    };
    let scope_abs: Vec<PathBuf> = scope
        .iter()
        .map(|s| root.join(s))
        .filter_map(|p| p.canonicalize().ok().or(Some(p)))
        .collect();
    files
        .iter()
        .filter(|f| {
            let canon = f.canonicalize().ok();
            scope_abs.iter().any(|allowed| {
                canon
                    .as_ref()
                    .map(|c| c.starts_with(allowed))
                    .unwrap_or_else(|| f.starts_with(allowed))
            })
        })
        .cloned()
        .collect()
}

fn build_module_map(root: &Path, files: &[PathBuf]) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for abs in files {
        let rel = match abs.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut parts: Vec<String> = rel
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(String::from))
            .collect();
        if parts.is_empty() {
            continue;
        }
        let last = parts.last().cloned().unwrap_or_default();
        if last == "__init__.py" {
            parts.pop();
        } else if let Some(idx) = last.rfind('.') {
            // Strip extension
            let stripped = last[..idx].to_string();
            *parts.last_mut().unwrap() = stripped;
        }
        if parts.is_empty() {
            continue;
        }
        let module_name = parts.join(".");
        let abs_str = abs.to_string_lossy().into_owned();
        out.insert(module_name.clone(), abs_str.clone());
        if module_name.contains('.') {
            if let Some(last) = parts.last() {
                out.insert(last.clone(), abs_str);
            }
        }
    }
    out
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyPlanContext>()?;
    m.add_function(wrap_pyfunction!(build_context, m)?)?;
    Ok(())
}
