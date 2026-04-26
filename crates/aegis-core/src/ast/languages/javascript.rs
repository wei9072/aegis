//! JavaScript adapter — handles `.js`, `.mjs`, `.cjs`, `.jsx`.
//!
//! TS / JS share most query patterns (`import x from "y"`,
//! `require("y")`); the queries live in separate `.scm` files only
//! to keep per-language ownership clean.

use tree_sitter::Language;

use crate::ast::adapter::LanguageAdapter;

pub fn language() -> Language {
    tree_sitter_javascript::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/javascript.scm");

pub struct JavaScriptAdapter;

impl LanguageAdapter for JavaScriptAdapter {
    fn name(&self) -> &'static str {
        "javascript"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".js", ".mjs", ".cjs", ".jsx"]
    }

    fn tree_sitter_language(&self) -> Language {
        language()
    }

    fn import_query(&self) -> &'static str {
        IMPORT_QUERY
    }
}
