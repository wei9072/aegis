use crate::ast::parser::{max_chain_depth, chain_depth};
use crate::ast::languages::python;

pub fn chain_depth_signal(filepath: &str) -> Result<f64, String> {
    let code = std::fs::read_to_string(filepath)
        .map_err(|e| e.to_string())?;
    let lang = python::language();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(lang).map_err(|e| e.to_string())?;
    let tree = parser.parse(&code, None)
        .ok_or("parse returned None")?;
    Ok(max_chain_depth(tree.root_node()) as f64)
}

