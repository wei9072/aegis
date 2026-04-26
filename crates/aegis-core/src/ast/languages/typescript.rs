use pyo3::prelude::*;
use tree_sitter::{Language, Parser, Query, QueryCursor};

pub fn language() -> Language {
    tree_sitter_typescript::language_typescript()
}

pub const IMPORT_QUERY: &str = "(import_statement source: (string) @import_path)";

#[pyfunction]
pub fn extract_ts_imports(code: &str) -> Vec<String> {
    let lang = language();
    let mut parser = Parser::new();
    if parser.set_language(lang).is_err() {
        return vec![];
    }
    let tree = match parser.parse(code, None) {
        Some(t) => t,
        None => return vec![],
    };
    let query = match Query::new(lang, IMPORT_QUERY) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut qc = QueryCursor::new();
    let code_bytes = code.as_bytes();
    let mut imports = Vec::new();
    for m in qc.matches(&query, tree.root_node(), code_bytes) {
        for cap in m.captures {
            let start = cap.node.start_byte();
            let end = cap.node.end_byte();
            if let Ok(text) = std::str::from_utf8(&code_bytes[start..end]) {
                let trimmed = text.trim_matches(|c| c == '\'' || c == '"' || c == '`');
                imports.push(trimmed.to_string());
            }
        }
    }
    imports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ts_imports() {
        let code = r#"
            import { something } from "./local/module";
            import React from 'react';
            import * as path from "node:path";
        "#;
        let imports = extract_ts_imports(code);
        assert!(imports.contains(&"./local/module".to_string()));
        assert!(imports.contains(&"react".to_string()));
        assert!(imports.contains(&"node:path".to_string()));
    }
}
