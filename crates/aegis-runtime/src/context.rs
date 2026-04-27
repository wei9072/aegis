//! `PlanContext` + workspace context-building helpers.
//!
//! Pure-Rust mirror of the V0.x Python `_build_context` +
//! `aegis.agents.planner.PlanContext` dataclass. Lives in
//! `aegis-runtime` so both the native Rust pipeline (V1.9 binary)
//! and the PyO3 wrappers in `aegis-pyshim::context` can share one
//! source of truth.
//!
//! The pure-Rust path uses `aegis-core` for signal extraction +
//! import scanning; `aegis-pyshim` simply wraps these types.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aegis_core::ast::parser::get_imports_native;
use aegis_core::ast::registry::LanguageRegistry;
use aegis_core::graph::cycle::has_cycle as graph_has_cycle;
use aegis_core::graph::dependency::DependencyGraph;
use aegis_core::signal_layer_pyapi::extract_signals_native;
use aegis_ir::PatchPlan;

use crate::executor::ExecutionResult;
use crate::validator::ValidationError;

/// Minimal Signal summary the prompt template renders. Mirrors the
/// V0.x `aegis.core.bindings.Signal` attribute surface (`name`,
/// `value`, `description`); `file_path` is dropped because the
/// containing map's key already records the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub value: f64,
    pub description: String,
}

/// Pure-Rust mirror of the V0.x `PlanContext` dataclass. The loop
/// updates `previous_*` fields per iteration before calling the
/// planner.
#[derive(Debug, Default)]
pub struct PlanContext {
    pub task: String,
    pub root: String,
    pub scope: Option<Vec<String>>,
    pub py_files: Vec<String>,
    /// signals[file_path] = list of signals on that file. BTreeMap
    /// keeps prompt rendering deterministic (path order).
    pub signals: BTreeMap<String, Vec<Signal>>,
    pub graph_edges: Vec<(String, String)>,
    pub has_cycle: bool,
    pub file_snippets: BTreeMap<String, String>,
    pub previous_plan: Option<PatchPlan>,
    pub previous_errors: Vec<ValidationError>,
    pub previous_result: Option<ExecutionResult>,
    pub previous_regressed: bool,
    pub previous_regression_detail: BTreeMap<String, f64>,
}

/// Parameters for `build_workspace_context`.
#[derive(Debug, Clone)]
pub struct ContextOptions {
    pub include_snippets: bool,
    pub max_snippets: usize,
    pub max_listed_files: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            include_snippets: true,
            // V0.x cap — keeps the prompt size bounded on large repos.
            max_snippets: 30,
            max_listed_files: 200,
        }
    }
}

/// Walk `root`, extract Ring 0.5 signals + imports + cycle info,
/// optionally collect file snippets, return a populated
/// `PlanContext`. Mirrors `aegis/runtime/pipeline.py::_build_context`
/// one-for-one.
pub fn build_workspace_context(
    task: &str,
    root: &Path,
    scope: Option<&[String]>,
    opts: &ContextOptions,
) -> PlanContext {
    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    let files_abs = discover_source_files(&root_abs);
    let files_rel: Vec<String> = files_abs
        .iter()
        .filter_map(|p| {
            p.strip_prefix(&root_abs)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        })
        .collect();

    let mut signals: BTreeMap<String, Vec<Signal>> = BTreeMap::new();
    for (abs, rel) in files_abs.iter().zip(files_rel.iter()) {
        let abs_str = abs.to_string_lossy();
        let sig_data = match extract_signals_native(&abs_str) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if sig_data.is_empty() {
            continue;
        }
        let sigs: Vec<Signal> = sig_data
            .into_iter()
            .map(|d| Signal {
                name: d.name,
                value: d.value,
                description: d.description,
            })
            .collect();
        signals.insert(rel.clone(), sigs);
    }

    let module_map = build_module_map(&root_abs, &files_abs);
    let mut edges: Vec<(String, String)> = Vec::new();
    for abs in &files_abs {
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
        let mut graph = DependencyGraph::new();
        graph.build_from_edges(edges.clone());
        graph_has_cycle(&graph)
    };

    let mut snippets: BTreeMap<String, String> = BTreeMap::new();
    if opts.include_snippets {
        let in_scope = scope_filter(&files_abs, &root_abs, scope);
        for abs in in_scope.iter().take(opts.max_snippets) {
            if let Ok(rel) = abs.strip_prefix(&root_abs) {
                if let Ok(body) = fs::read_to_string(abs) {
                    snippets.insert(rel.to_string_lossy().into_owned(), body);
                }
            }
        }
    }

    PlanContext {
        task: task.to_string(),
        root: root_abs.to_string_lossy().into_owned(),
        scope: scope.map(<[String]>::to_vec),
        py_files: files_rel.into_iter().take(opts.max_listed_files).collect(),
        signals,
        graph_edges: edges,
        has_cycle,
        file_snippets: snippets,
        previous_plan: None,
        previous_errors: Vec::new(),
        previous_result: None,
        previous_regressed: false,
        previous_regression_detail: BTreeMap::new(),
    }
}

// ---------- helpers ----------

pub fn discover_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let allowed: Vec<&'static str> = LanguageRegistry::global().extensions().to_vec();
    walk(root, &allowed, &mut out);
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
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            // Skip hidden (.git, .aegis, etc.) and __pycache__ — same
            // set the V0.x Python `_discover_py_files` skipped.
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

pub fn scope_filter(
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

pub fn build_module_map(
    root: &Path,
    files: &[PathBuf],
) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discover_source_files_skips_hidden_and_cache() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.py"), "x = 1\n").unwrap();
        fs::create_dir(td.path().join(".hidden")).unwrap();
        fs::write(td.path().join(".hidden/secret.py"), "x = 1\n").unwrap();
        fs::create_dir(td.path().join("__pycache__")).unwrap();
        fs::write(td.path().join("__pycache__/x.cpython-312.pyc"), b"junk").unwrap();
        let files = discover_source_files(td.path());
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(names, vec!["a.py"]);
    }

    #[test]
    fn build_workspace_context_picks_up_signals_and_snippets() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.py"), "import os\nx = os.path.join(1, 2)\n")
            .unwrap();
        let ctx = build_workspace_context(
            "demo task",
            td.path(),
            None,
            &ContextOptions::default(),
        );
        assert_eq!(ctx.task, "demo task");
        assert!(ctx.py_files.contains(&"a.py".to_string()));
        assert!(ctx.signals.contains_key("a.py"));
        let sigs = &ctx.signals["a.py"];
        assert!(sigs.iter().any(|s| s.name == "fan_out"));
        assert!(sigs.iter().any(|s| s.name == "max_chain_depth"));
        assert!(ctx.file_snippets.contains_key("a.py"));
    }

    #[test]
    fn build_module_map_handles_packages_and_init() {
        let td = tempdir().unwrap();
        let pkg = td.path().join("pkg");
        fs::create_dir(&pkg).unwrap();
        fs::write(pkg.join("__init__.py"), "").unwrap();
        fs::write(pkg.join("mod.py"), "").unwrap();
        let files = discover_source_files(td.path());
        let map = build_module_map(td.path(), &files);
        assert!(map.contains_key("pkg")); // from __init__.py
        assert!(map.contains_key("pkg.mod"));
        // The bare-name shortcut for nested modules:
        assert!(map.contains_key("mod"));
    }

    #[test]
    fn scope_filter_keeps_only_files_under_scope() {
        let td = tempdir().unwrap();
        fs::create_dir(td.path().join("sub")).unwrap();
        fs::write(td.path().join("a.py"), "").unwrap();
        fs::write(td.path().join("sub/b.py"), "").unwrap();
        let files = discover_source_files(td.path());
        let kept = scope_filter(&files, td.path(), Some(&["sub".to_string()]));
        let names: Vec<_> = kept
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(names, vec!["b.py"]);
    }
}
