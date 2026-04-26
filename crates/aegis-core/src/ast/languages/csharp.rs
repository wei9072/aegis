use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_c_sharp::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/csharp.scm");

pub struct CSharpAdapter;

impl LanguageAdapter for CSharpAdapter {
    fn name(&self) -> &'static str {
        "csharp"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".cs"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
