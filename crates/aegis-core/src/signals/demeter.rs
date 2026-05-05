//! Max-chain-depth signal — language-agnostic via the registry.

use crate::ast::parsed_file::ParsedFile;

/// Compute max chain depth from a pre-parsed file. Defers to the
/// adapter's per-language walker so non-OO syntaxes can override.
pub fn chain_depth(parsed: &ParsedFile<'_>) -> f64 {
    parsed.adapter().max_chain_depth(parsed.root_node()) as f64
}
