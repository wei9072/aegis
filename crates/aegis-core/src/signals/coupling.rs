//! Fan-out signal — count distinct external imports.
//!
//! Language-agnostic via the registry. Files with no registered
//! adapter return 0 (a non-opinion); higher layers decide whether
//! to surface that as "unsupported language" or just skip.

use tree_sitter::{Query, QueryCursor};

use crate::ast::parsed_file::ParsedFile;
use crate::ast::registry::LanguageRegistry;
use crate::ir::model::IrNode;

/// Count unique import targets (fan-out) from pre-parsed IR nodes.
pub fn fan_out_from_ir(nodes: &[IrNode]) -> usize {
    nodes.iter().filter(|n| n.kind == "dependency").count()
}

/// Count unique imports from source code given an explicit adapter.
/// Used internally for tests; callers typically go through
/// `fan_out_signal` which dispatches by file extension.
pub fn fan_out_from_code_with(code: &str, adapter: &dyn crate::ast::LanguageAdapter) -> usize {
    let lang = adapter.tree_sitter_language();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() {
        return 0;
    }
    let tree = match parser.parse(code, None) {
        Some(t) => t,
        None => return 0,
    };
    let query = match Query::new(lang, adapter.import_query()) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    for m in qc.matches(&query, tree.root_node(), code.as_bytes()) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                seen.insert(adapter.normalize_import(text));
            }
        }
    }
    seen.len()
}

pub fn fan_out_signal(filepath: &str) -> Result<f64, String> {
    let code = std::fs::read_to_string(filepath).map_err(|e| e.to_string())?;
    let registry = LanguageRegistry::global();
    let Some(adapter) = registry.for_path(filepath) else {
        return Ok(0.0);
    };
    Ok(fan_out_from_code_with(&code, adapter) as f64)
}

/// Layer 1-shared variant — count unique imports from a pre-parsed
/// `ParsedFile`. No re-parse, no disk read. Returns 0 when the
/// adapter's import query fails to compile (defensive — the registry
/// meta-test guards this at startup, so it shouldn't fire in practice).
pub fn fan_out_from_parsed(parsed: &ParsedFile<'_>) -> usize {
    let adapter = parsed.adapter();
    let lang = adapter.tree_sitter_language();
    let query = match Query::new(lang, adapter.import_query()) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    let src = parsed.source_bytes();
    for m in qc.matches(&query, parsed.root_node(), src) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(src) {
                seen.insert(adapter.normalize_import(text));
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::languages::javascript::JavaScriptAdapter;
    use crate::ast::languages::python::PythonAdapter;
    use crate::ast::languages::typescript::TypeScriptAdapter;

    #[test]
    fn python_relative_import_is_counted() {
        let code = "from . import foo\nfrom .bar import baz\nimport os\n";
        let n = fan_out_from_code_with(code, &PythonAdapter);
        assert!(n >= 2, "expected relative imports + os; got fan_out={n}");
    }

    #[test]
    fn typescript_export_from_is_counted() {
        let code = "export { a } from './a';\nexport * from './b';\n";
        let n = fan_out_from_code_with(code, &TypeScriptAdapter);
        assert!(n >= 2, "expected ./a + ./b; got fan_out={n}");
    }

    #[test]
    fn typescript_dynamic_import_is_counted() {
        let code = "const m = import('./dyn');\n";
        let n = fan_out_from_code_with(code, &TypeScriptAdapter);
        assert!(n >= 1, "expected dynamic import('./dyn'); got fan_out={n}");
    }

    #[test]
    fn javascript_export_from_and_dynamic_import_counted() {
        let code = "export { a } from './a';\nimport('./b');\n";
        let n = fan_out_from_code_with(code, &JavaScriptAdapter);
        assert!(n >= 2, "expected ./a + ./b; got fan_out={n}");
    }
}
