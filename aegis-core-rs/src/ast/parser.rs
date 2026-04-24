use pyo3::prelude::*;
use std::fs;
use tree_sitter::{Node, Parser, Query, QueryCursor};

use crate::ast::languages::python;

#[pyclass]
#[derive(Clone)]
pub struct AstMetrics {
    #[pyo3(get)]
    pub has_syntax_error: bool,
    #[pyo3(get)]
    pub fan_out: usize,
    #[pyo3(get)]
    pub max_chain_depth: usize,
}

#[pyfunction]
pub fn analyze_file(filepath: &str) -> PyResult<AstMetrics> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let mut parser = Parser::new();
    let lang = python::language();
    parser.set_language(lang).unwrap();

    let tree = parser.parse(&code, None).unwrap();
    let root_node = tree.root_node();
    let has_syntax_error = root_node.has_error();

    let mut fan_out = 0;
    if let Ok(query) = Query::new(lang, python::IMPORT_QUERY) {
        let mut qc = QueryCursor::new();
        let mut seen = std::collections::HashSet::new();
        for m in qc.matches(&query, root_node, code.as_bytes()) {
            for cap in m.captures {
                if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                    seen.insert(text.to_string());
                }
            }
        }
        fan_out = seen.len();
    }

    let max_chain_depth = max_chain_depth(root_node);

    Ok(AstMetrics { has_syntax_error, fan_out, max_chain_depth })
}

#[pyfunction]
pub fn get_imports(filepath: &str) -> PyResult<Vec<String>> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let mut parser = Parser::new();
    let lang = python::language();
    parser.set_language(lang)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    let tree = parser.parse(&code, None)
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("parse returned None"))?;
    let root_node = tree.root_node();

    let query = Query::new(lang, python::IMPORT_QUERY)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    for m in qc.matches(&query, root_node, code.as_bytes()) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                seen.insert(text.to_string());
            }
        }
    }

    let mut result: Vec<String> = seen.into_iter().collect();
    result.sort();
    Ok(result)
}

pub fn max_chain_depth(node: Node) -> usize {
    let mut max = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute" || child.kind() == "call" {
            max = max.max(chain_depth(child));
        }
        max = max.max(max_chain_depth(child));
    }
    max
}

pub fn chain_depth(node: Node) -> usize {
    match node.kind() {
        "attribute" => node
            .child_by_field_name("object")
            .map(|obj| 1 + chain_depth(obj))
            .unwrap_or(1),
        "call" => node
            .child_by_field_name("function")
            .map(chain_depth)
            .unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_get_imports_returns_sorted_list() {
        let code = b"import os\nimport sys\nfrom mymodule import Foo\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let result = get_imports(tmp.path().to_str().unwrap()).unwrap();
        assert!(result.contains(&"os".to_string()));
        assert!(result.contains(&"mymodule".to_string()));
        assert_eq!(result, { let mut s = result.clone(); s.sort(); s });
    }

    #[test]
    fn test_get_imports_deduplication() {
        let code = b"import os\nimport os\nfrom os import path\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let result = get_imports(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(result.iter().filter(|s| s.as_str() == "os").count(), 1);
    }
}
