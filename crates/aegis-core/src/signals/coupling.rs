use tree_sitter::{Query, QueryCursor};
use crate::ast::languages::python;
use crate::ir::model::IrNode;

/// Count unique import targets (fan-out) from pre-parsed IR nodes.
pub fn fan_out_from_ir(nodes: &[IrNode]) -> usize {
    nodes.iter().filter(|n| n.kind == "dependency").count()
}

/// Count unique imports directly from source code bytes.
pub fn fan_out_from_code(code: &str) -> usize {
    let lang = python::language();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() {
        return 0;
    }
    let tree = match parser.parse(code, None) {
        Some(t) => t,
        None => return 0,
    };
    let query = match Query::new(lang, python::IMPORT_QUERY) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    for m in qc.matches(&query, tree.root_node(), code.as_bytes()) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                seen.insert(text.to_string());
            }
        }
    }
    seen.len()
}

pub fn fan_out_signal(filepath: &str) -> Result<f64, String> {
    let code = std::fs::read_to_string(filepath)
        .map_err(|e| e.to_string())?;
    Ok(fan_out_from_code(&code) as f64)
}
