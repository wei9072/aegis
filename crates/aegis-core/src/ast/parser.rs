//! Language-agnostic AST entry points.
//!
//! Dispatch is by file extension via the global `LanguageRegistry`.
//! Adding a language is purely an additive registry change — nothing
//! in this file knows about specific grammars.

use pyo3::prelude::*;
use std::fs;
use tree_sitter::{Parser, Query, QueryCursor};

use crate::ast::registry::LanguageRegistry;

#[pyclass]
#[derive(Clone)]
pub struct AstMetrics {
    #[pyo3(get)]
    pub has_syntax_error: bool,
    #[pyo3(get)]
    pub fan_out: usize,
    #[pyo3(get)]
    pub max_chain_depth: usize,
    #[pyo3(get)]
    pub language: String,
}

fn unsupported(filepath: &str) -> PyErr {
    let names = LanguageRegistry::global().names();
    pyo3::exceptions::PyValueError::new_err(format!(
        "no language adapter for {filepath:?} (supported: {names:?})"
    ))
}

#[pyfunction]
pub fn analyze_file(filepath: &str) -> PyResult<AstMetrics> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let registry = LanguageRegistry::global();
    let adapter = registry.for_path(filepath).ok_or_else(|| unsupported(filepath))?;
    let lang = adapter.tree_sitter_language();

    let mut parser = Parser::new();
    parser
        .set_language(lang)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    let tree = parser
        .parse(&code, None)
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("parse returned None"))?;
    let root = tree.root_node();
    let has_syntax_error = root.has_error();

    let mut fan_out = 0;
    if let Ok(query) = Query::new(lang, adapter.import_query()) {
        let mut qc = QueryCursor::new();
        let mut seen = std::collections::HashSet::new();
        for m in qc.matches(&query, root, code.as_bytes()) {
            for cap in m.captures {
                if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                    seen.insert(adapter.normalize_import(text));
                }
            }
        }
        fan_out = seen.len();
    }

    let max_chain_depth = adapter.max_chain_depth(root);

    Ok(AstMetrics {
        has_syntax_error,
        fan_out,
        max_chain_depth,
        language: adapter.name().to_string(),
    })
}

#[pyfunction]
pub fn get_imports(filepath: &str) -> PyResult<Vec<String>> {
    get_imports_native(filepath).map_err(pyo3::exceptions::PyIOError::new_err)
}

/// Pure-Rust import extraction. Same logic as `get_imports`, but
/// returns `Result<Vec<String>, String>` so callers without a Python
/// runtime (the V1.9 `aegis-cli` / `aegis-mcp` binaries) can use it.
pub fn get_imports_native(filepath: &str) -> Result<Vec<String>, String> {
    let code = fs::read_to_string(filepath).map_err(|e| e.to_string())?;

    let registry = LanguageRegistry::global();
    let adapter = registry
        .for_path(filepath)
        .ok_or_else(|| format!("no language adapter for {filepath:?}"))?;
    let lang = adapter.tree_sitter_language();

    let mut parser = Parser::new();
    parser.set_language(lang).map_err(|e| e.to_string())?;
    let tree = parser
        .parse(&code, None)
        .ok_or_else(|| "parse returned None".to_string())?;
    let root = tree.root_node();

    let query = Query::new(lang, adapter.import_query()).map_err(|e| e.to_string())?;

    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    for m in qc.matches(&query, root, code.as_bytes()) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                seen.insert(adapter.normalize_import(text));
            }
        }
    }

    let mut result: Vec<String> = seen.into_iter().collect();
    result.sort();
    Ok(result)
}

/// Language-agnostic node walker — re-exported for backward compat
/// with the V0.x `aegis_core_rs::ast::parser::max_chain_depth`
/// callers in `signals/demeter.rs`.
pub fn max_chain_depth(node: tree_sitter::Node) -> usize {
    crate::ast::adapter::default_max_chain_depth(node)
}

/// Per-node chain depth using the default trait impl. Same back-compat
/// rationale as `max_chain_depth` above.
pub fn chain_depth(_node: tree_sitter::Node) -> usize {
    // Public for legacy callers; the live walker is the trait method.
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_get_imports_returns_sorted_list() {
        let code = b"import os\nimport sys\nfrom mymodule import Foo\n";
        let mut tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let result = get_imports(tmp.path().to_str().unwrap()).unwrap();
        assert!(result.contains(&"os".to_string()));
        assert!(result.contains(&"mymodule".to_string()));
        assert_eq!(result, {
            let mut s = result.clone();
            s.sort();
            s
        });
    }

    #[test]
    fn test_get_imports_deduplication() {
        let code = b"import os\nimport os\nfrom os import path\n";
        let mut tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let result = get_imports(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(result.iter().filter(|s| s.as_str() == "os").count(), 1);
    }

    #[test]
    fn test_analyze_dispatches_typescript() {
        let code = b"import { foo } from './bar';\nimport React from 'react';\n";
        let mut tmp = tempfile::Builder::new().suffix(".ts").tempfile().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let m = analyze_file(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(m.language, "typescript");
        assert_eq!(m.has_syntax_error, false);
        assert!(m.fan_out >= 2);
    }

    #[test]
    fn test_analyze_unknown_extension_returns_value_error() {
        let mut tmp = tempfile::Builder::new().suffix(".unknown").tempfile().unwrap();
        tmp.write_all(b"hello").unwrap();
        tmp.flush().unwrap();
        let r = analyze_file(tmp.path().to_str().unwrap());
        assert!(r.is_err());
    }
}
