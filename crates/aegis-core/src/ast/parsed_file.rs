//! Layer 1 infrastructure — shared parsed-file abstraction.
//!
//! Wraps `tree_sitter::Tree` together with the source bytes and the
//! adapter that produced it. Lets every downstream consumer (Ring 0.5
//! signals, Ring 0.7 security, Ring R2 workspace) share the same
//! parse instead of each calling `Parser::new()` independently.
//!
//! `parse()` always returns the tree (when an adapter exists for the
//! extension), even when `tree.root_node().has_error()` is true.
//! Tree-sitter is error-tolerant by design; whether a parse-error
//! file should be flagged is a *finding* decision left to consumers,
//! not a hard short-circuit baked into the parse layer.

use crate::ast::adapter::LanguageAdapter;
use crate::ast::registry::LanguageRegistry;

/// Output of a successful parse — tree + source + the adapter that
/// produced it. Cheap to pass by reference; nothing here is cloned.
pub struct ParsedFile<'src> {
    tree: tree_sitter::Tree,
    source: &'src str,
    language_name: &'static str,
}

impl<'src> ParsedFile<'src> {
    /// The parsed tree. Walk this for any AST analysis.
    pub fn tree(&self) -> &tree_sitter::Tree {
        &self.tree
    }

    /// Convenience: the tree's root node.
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    /// Source code as `&str` — needed for `node.utf8_text(...)`.
    pub fn source(&self) -> &'src str {
        self.source
    }

    /// Source code as `&[u8]` — what `node.utf8_text(...)` actually
    /// expects. Most callers pass this rather than the `&str`.
    pub fn source_bytes(&self) -> &'src [u8] {
        self.source.as_bytes()
    }

    /// Stable language name (e.g. `"python"`, `"typescript"`). Use
    /// this when you need to dispatch on the language without holding
    /// a borrow on the adapter.
    pub fn language_name(&self) -> &'static str {
        self.language_name
    }

    /// Look up the adapter that produced this tree. Re-resolves
    /// against the global registry — cheap, but if you need the
    /// adapter many times, cache the return value locally.
    pub fn adapter(&self) -> &dyn LanguageAdapter {
        LanguageRegistry::global()
            .for_name(self.language_name)
            .expect("adapter must remain registered for the lifetime of a ParsedFile")
    }

    /// Whether the parse contains any ERROR / MISSING nodes. Reported
    /// for callers that want to expose syntax-validity as a finding;
    /// the parse layer itself does not act on this.
    pub fn has_syntax_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }
}

/// Parse `source` as the language inferred from `path`'s extension.
///
/// Returns `None` only when:
///   - the extension has no registered `LanguageAdapter` (not our
///     language — caller should treat this as "no opinion"), or
///   - tree-sitter `set_language` / `parse` returns an error (rare;
///     in practice means out-of-memory or a corrupt grammar).
///
/// **Always returns `Some` when the language is registered, even if
/// the source has syntax errors.** Use `ParsedFile::has_syntax_errors`
/// if you need to know.
pub fn parse<'src>(path: &str, source: &'src str) -> Option<ParsedFile<'src>> {
    let registry = LanguageRegistry::global();
    let adapter = registry.for_path(path)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(adapter.tree_sitter_language()).ok()?;
    let tree = parser.parse(source, None)?;
    Some(ParsedFile {
        tree,
        source,
        language_name: adapter.name(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_python() {
        let pf = parse("foo.py", "x = 1\n").expect("python parses");
        assert!(!pf.has_syntax_errors());
        assert_eq!(pf.language_name(), "python");
        assert_eq!(pf.source(), "x = 1\n");
    }

    #[test]
    fn parses_broken_python_without_short_circuit() {
        // Tree-sitter is error-tolerant; we still get a tree back even
        // when the source is unfinished. The parse layer never decides
        // to BLOCK — that's a finding-layer call.
        let pf = parse("broken.py", "def foo(\n").expect("still returns a tree");
        assert!(pf.has_syntax_errors(), "ERROR/MISSING node expected");
    }

    #[test]
    fn unknown_extension_returns_none() {
        // .xyz has no adapter — caller should treat as "no opinion".
        assert!(parse("notes.xyz", "anything").is_none());
    }

    #[test]
    fn adapter_lookup_round_trips() {
        let pf = parse("lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n")
            .expect("rust parses");
        assert_eq!(pf.adapter().name(), "rust");
    }

    #[test]
    fn root_node_is_walkable() {
        let pf = parse("a.ts", "const x: number = 1;\n").expect("ts parses");
        let root = pf.root_node();
        // Sanity: TS program root has named children.
        assert!(root.named_child_count() > 0);
    }
}
