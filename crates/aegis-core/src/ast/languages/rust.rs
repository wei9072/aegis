use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_rust::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/rust.scm");

pub struct RustAdapter;

impl LanguageAdapter for RustAdapter {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".rs"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }

    /// Rust paths are `::`-separated and may carry `as alias` or
    /// brace-list syntax. For cycle detection we only need the
    /// leftmost segment (the crate / top-level module name), so
    /// strip everything after the first `::` and any trailing
    /// `as ...` / whitespace.
    fn normalize_import(&self, raw: &str) -> String {
        let trimmed = raw.trim();
        let head = trimmed.split("::").next().unwrap_or(trimmed);
        head.split_whitespace().next().unwrap_or(head).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::{Parser, Query, QueryCursor};

    fn extract_imports(code: &str) -> Vec<String> {
        let lang = language();
        let mut parser = Parser::new();
        parser.set_language(lang).unwrap();
        let tree = parser.parse(code, None).unwrap();
        let query = Query::new(lang, IMPORT_QUERY).unwrap();
        let mut qc = QueryCursor::new();
        let bytes = code.as_bytes();
        let adapter = RustAdapter;
        let mut out = Vec::new();
        for m in qc.matches(&query, tree.root_node(), bytes) {
            for cap in m.captures {
                let txt = std::str::from_utf8(
                    &bytes[cap.node.start_byte()..cap.node.end_byte()],
                )
                .unwrap();
                out.push(adapter.normalize_import(txt));
            }
        }
        out
    }

    #[test]
    fn extracts_simple_use() {
        let imports = extract_imports("use foo;\n");
        assert!(imports.contains(&"foo".to_string()));
    }

    #[test]
    fn extracts_scoped_use_takes_leftmost() {
        let imports = extract_imports("use std::io::Read;\n");
        assert!(imports.contains(&"std".to_string()));
    }

    #[test]
    fn extracts_use_list() {
        let imports = extract_imports("use std::io::{Read, Write};\n");
        assert!(imports.contains(&"std".to_string()));
    }

    #[test]
    fn extracts_use_as_alias() {
        let imports = extract_imports("use foo::bar as baz;\n");
        assert!(imports.contains(&"foo".to_string()));
        assert!(!imports.iter().any(|s| s == "baz"));
    }

    #[test]
    fn extracts_mod_item() {
        let imports = extract_imports("mod helpers;\n");
        assert!(imports.contains(&"helpers".to_string()));
    }

    #[test]
    fn extracts_extern_crate() {
        let imports = extract_imports("extern crate serde;\n");
        assert!(imports.contains(&"serde".to_string()));
    }

    #[test]
    fn empty_file_returns_no_imports() {
        assert!(extract_imports("").is_empty());
    }

    #[test]
    fn syntax_error_does_not_panic() {
        let _ = extract_imports("fn broken(\n");
    }
}
