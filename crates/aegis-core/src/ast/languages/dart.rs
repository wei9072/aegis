//! Dart adapter — handles `.dart`. Backed by the community-maintained
//! `tree-sitter-dart` crate; covers Flutter projects via the same
//! adapter (Flutter is just Dart + a framework).

use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_dart::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/dart.scm");

pub struct DartAdapter;

impl LanguageAdapter for DartAdapter {
    fn name(&self) -> &'static str {
        "dart"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".dart"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
