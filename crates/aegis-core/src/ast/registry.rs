//! Single-source-of-truth registry of every `LanguageAdapter`.
//!
//! New language? Add a `Box::new(<Lang>Adapter)` line to
//! `default_set` and nothing else in this file changes. Dispatch is
//! by file extension via `for_path`.

use std::sync::OnceLock;

use crate::ast::adapter::LanguageAdapter;
use crate::ast::languages;

static REGISTRY: OnceLock<LanguageRegistry> = OnceLock::new();

pub struct LanguageRegistry {
    adapters: Vec<Box<dyn LanguageAdapter>>,
}

impl LanguageRegistry {
    pub fn global() -> &'static LanguageRegistry {
        REGISTRY.get_or_init(LanguageRegistry::default_set)
    }

    /// Order-independent (dispatch is by extension) but kept in
    /// roughly the order they were added to the project — Python
    /// first because that's the legacy reference implementation.
    fn default_set() -> Self {
        Self {
            adapters: vec![
                Box::new(languages::python::PythonAdapter),
                Box::new(languages::typescript::TypeScriptAdapter),
                Box::new(languages::javascript::JavaScriptAdapter),
                Box::new(languages::go::GoAdapter),
                Box::new(languages::java::JavaAdapter),
                Box::new(languages::csharp::CSharpAdapter),
                Box::new(languages::php::PhpAdapter),
                Box::new(languages::swift::SwiftAdapter),
                Box::new(languages::kotlin::KotlinAdapter),
                Box::new(languages::dart::DartAdapter),
            ],
        }
    }

    pub fn for_path(&self, path: &str) -> Option<&dyn LanguageAdapter> {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))?;
        self.adapters
            .iter()
            .find(|a| a.extensions().iter().any(|x| *x == ext))
            .map(|b| b.as_ref())
    }

    pub fn for_name(&self, name: &str) -> Option<&dyn LanguageAdapter> {
        self.adapters.iter().find(|a| a.name() == name).map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.name()).collect()
    }

    pub fn extensions(&self) -> Vec<&'static str> {
        self.adapters.iter().flat_map(|a| a.extensions().iter().copied()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dispatches_python_by_extension() {
        let r = LanguageRegistry::global();
        assert!(r.for_path("foo.py").is_some());
        assert_eq!(r.for_path("foo.py").unwrap().name(), "python");
    }

    #[test]
    fn registry_returns_none_for_unknown_extension() {
        let r = LanguageRegistry::global();
        assert!(r.for_path("foo.unknown").is_none());
    }

    #[test]
    fn every_adapter_parses_empty_file_without_panicking() {
        // Cross-language meta-test from multi_language_plan.md
        // testing strategy: registered adapters must at minimum
        // parse the empty file for their primary extension without
        // crashing. Catches "forgot to register" and broken grammar
        // bindings simultaneously.
        let r = LanguageRegistry::global();
        for a in &r.adapters {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(a.tree_sitter_language())
                .unwrap_or_else(|e| panic!("set_language failed for {}: {e}", a.name()));
            let tree = parser
                .parse("", None)
                .unwrap_or_else(|| panic!("parse(empty) returned None for {}", a.name()));
            // Empty file: no error sentinel from the grammar.
            assert!(
                !tree.root_node().has_error(),
                "{} grammar reports error on empty file",
                a.name()
            );
        }
    }

    #[test]
    fn every_adapter_compiles_its_import_query() {
        let r = LanguageRegistry::global();
        for a in &r.adapters {
            let lang = a.tree_sitter_language();
            tree_sitter::Query::new(lang, a.import_query())
                .unwrap_or_else(|e| panic!("import_query bad for {}: {e}", a.name()));
        }
    }
}
