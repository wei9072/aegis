//! `LanguageAdapter` trait + the language-agnostic chain-depth walker.
//!
//! Each supported language ships an adapter struct in
//! `crates/aegis-core/src/ast/languages/<lang>.rs` and registers
//! itself in `LanguageRegistry::default_set()`.
//!
//! The trait is intentionally narrow: every method maps to a single
//! tree-sitter primitive so adding a language doesn't require
//! re-deriving design decisions.

use tree_sitter::{Language, Node};

pub trait LanguageAdapter: Send + Sync {
    /// Stable name — "python", "typescript", "go". Used in error
    /// messages, registry queries, and the supported-languages table
    /// shown to users.
    fn name(&self) -> &'static str;

    /// File extensions this adapter handles, lowercase, with leading
    /// dot — `[".py"]` / `[".ts", ".tsx"]`.
    fn extensions(&self) -> &'static [&'static str];

    /// The tree-sitter grammar entry point.
    fn tree_sitter_language(&self) -> Language;

    /// Tree-sitter S-expression query: capture every imported /
    /// required module identifier as `@import`. The captured node's
    /// utf8 text is taken verbatim (with leading/trailing quotes /
    /// backticks stripped — see `import_text_from_capture`).
    fn import_query(&self) -> &'static str;

    /// Walk the AST and return the longest method-chain depth.
    /// Default works on member-access / call shapes — overridable per
    /// language for non-OO syntaxes.
    fn max_chain_depth(&self, root: Node) -> usize {
        default_max_chain_depth(root)
    }

    /// Override when an import capture wraps the literal in a string
    /// (TS / JS / Go all do this). Default returns the bytes as-is
    /// after stripping quotes/backticks.
    fn normalize_import(&self, raw: &str) -> String {
        raw.trim_matches(|c| c == '\'' || c == '"' || c == '`').to_string()
    }
}

/// Default `max_chain_depth` — counts nested member-access / call
/// chains.
///
/// The walker is intentionally union-of-known-shapes across all
/// supported tree-sitter grammars. Per-language overrides live on
/// `LanguageAdapter::max_chain_depth` for languages whose AST shape
/// doesn't fit the default (functional, non-OO, or where the chain
/// concept doesn't apply).
pub fn default_max_chain_depth(node: Node) -> usize {
    let mut max = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_chain_node(child.kind()) {
            max = max.max(default_chain_depth(child));
        }
        max = max.max(default_max_chain_depth(child));
    }
    max
}

fn is_chain_node(kind: &str) -> bool {
    matches!(
        kind,
        // Python
        "attribute" | "call" | "subscript"
        // TypeScript / JavaScript
        | "member_expression" | "call_expression" | "subscript_expression"
        // Java
        | "method_invocation" | "field_access"
        // Go
        | "selector_expression" | "index_expression"
        // C#
        | "invocation_expression" | "member_access_expression"
        | "element_access_expression"
        // PHP
        | "member_call_expression" | "scoped_call_expression"
        | "member_access_expression_php" | "function_call_expression"
        | "scoped_property_access_expression"
        // Swift / Kotlin / Dart
        | "navigation_expression"
        | "selector"
        // Rust
        | "field_expression"
        | "try_expression"
    )
}

fn default_chain_depth(node: Node) -> usize {
    let kind = node.kind();
    // Member-access / field-access shapes — recurse into the
    // receiver and add 1.
    if matches!(
        kind,
        "attribute"
            | "member_expression"
            | "field_access"
            | "field_expression"
            | "selector_expression"
            | "navigation_expression"
            | "member_access_expression"
            | "scoped_property_access_expression"
            | "selector"
            | "try_expression"
    ) {
        // Most grammars expose the receiver as field "object" or
        // "operand" or "expression". Rust `field_expression` uses
        // "value"; fall back to the first named child for grammars
        // without explicit field names.
        let recv = node
            .child_by_field_name("object")
            .or_else(|| node.child_by_field_name("operand"))
            .or_else(|| node.child_by_field_name("expression"))
            .or_else(|| node.child_by_field_name("scope"))
            .or_else(|| node.child_by_field_name("value"))
            .or_else(|| node.named_child(0));
        return recv.map(|n| 1 + default_chain_depth(n)).unwrap_or(1);
    }

    // Call shapes — calls don't add to the chain count, they
    // unwrap to their target. `a.b().c` is depth 3, same as
    // `a.b.c`. The receiver is at the call's "function"
    // (Python), or its "object" (Java method_invocation), or
    // "expression" (C#), or "scope" (PHP), depending on grammar.
    if matches!(
        kind,
        "call"
            | "call_expression"
            | "method_invocation"
            | "invocation_expression"
            | "member_call_expression"
            | "scoped_call_expression"
            | "function_call_expression"
            | "subscript"
            | "subscript_expression"
            | "index_expression"
            | "element_access_expression"
    ) {
        // For calls, the receiver chain is what we want — try a
        // few field names in priority order matching different
        // grammars' conventions.
        let receiver = node
            .child_by_field_name("object")
            .or_else(|| node.child_by_field_name("function"))
            .or_else(|| node.child_by_field_name("expression"))
            .or_else(|| node.child_by_field_name("scope"))
            .or_else(|| node.child_by_field_name("operand"))
            .or_else(|| node.named_child(0));
        return receiver.map(default_chain_depth).unwrap_or(0);
    }

    0
}
