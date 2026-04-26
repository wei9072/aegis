use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_java::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/java.scm");

pub struct JavaAdapter;

impl LanguageAdapter for JavaAdapter {
    fn name(&self) -> &'static str {
        "java"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".java"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
