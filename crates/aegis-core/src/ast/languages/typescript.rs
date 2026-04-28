use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    // tsx grammar parses both `.ts` and `.tsx`, so a single adapter
    // can cover both extensions without runtime branching.
    tree_sitter_typescript::language_tsx()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/typescript.scm");

pub struct TypeScriptAdapter;

impl LanguageAdapter for TypeScriptAdapter {
    fn name(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".ts", ".tsx", ".mts", ".cts"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
