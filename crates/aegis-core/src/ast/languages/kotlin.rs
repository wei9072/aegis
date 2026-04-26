//! Kotlin adapter — handles `.kt`, `.kts`. Backed by the
//! community-maintained `tree-sitter-kotlin` crate; the registry
//! meta-test pins that empty-file parse + import-query compile
//! both pass before declaring the adapter shipped.

use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_kotlin::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/kotlin.scm");

pub struct KotlinAdapter;

impl LanguageAdapter for KotlinAdapter {
    fn name(&self) -> &'static str {
        "kotlin"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".kt", ".kts"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
