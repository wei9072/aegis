//! Max-chain-depth signal — language-agnostic via the registry.

use crate::ast::parsed_file::ParsedFile;
use crate::ast::registry::LanguageRegistry;

pub fn chain_depth_signal(filepath: &str) -> Result<f64, String> {
    let code = std::fs::read_to_string(filepath).map_err(|e| e.to_string())?;
    let registry = LanguageRegistry::global();
    let Some(adapter) = registry.for_path(filepath) else {
        return Ok(0.0);
    };
    let lang = adapter.tree_sitter_language();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(lang).map_err(|e| e.to_string())?;
    let tree = parser.parse(&code, None).ok_or("parse returned None")?;
    Ok(adapter.max_chain_depth(tree.root_node()) as f64)
}

/// Layer 1-shared variant — compute max chain depth from a pre-parsed
/// `ParsedFile`. Defers to the adapter's per-language walker.
pub fn chain_depth_from_parsed(parsed: &ParsedFile<'_>) -> f64 {
    parsed.adapter().max_chain_depth(parsed.root_node()) as f64
}
