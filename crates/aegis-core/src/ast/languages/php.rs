//! PHP adapter — handles `.php`, `.phtml`, `.php5`, `.php7`, `.phps`.
//!
//! Mixed-content PHP/HTML is deferred (multi_language_plan.md P6);
//! V1.7 treats files as pure PHP via `tree_sitter_php::language_php`.

use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_php::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/php.scm");

pub struct PhpAdapter;

impl LanguageAdapter for PhpAdapter {
    fn name(&self) -> &'static str {
        "php"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".php", ".phtml", ".php5", ".php7", ".phps"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
