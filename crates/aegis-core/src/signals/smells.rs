//! AST-walked structural smell counters — language-agnostic.
//!
//! All signals here share a single traversal pass per file. Each
//! metric counts a kind of pattern LLMs frequently produce that
//! Ring 0 / Ring 0.5 (fan_out + chain_depth) miss entirely.
//!
//! Discipline alignment: every counter participates in cost-aware
//! regression — we never block on absolute value, only on
//! "new > old". So existing TODOs / empty catches / etc. in a file
//! never get aegis to retroactively complain; only LLM additions do.

use tree_sitter::Node;

use crate::ast::registry::LanguageRegistry;

/// Output of a single-file structural smell scan.
#[derive(Debug, Default, Clone)]
pub struct SmellCounts {
    pub empty_handler_count: f64,
    pub unfinished_marker_count: f64,
    pub unreachable_stmt_count: f64,
    pub cyclomatic_complexity: f64,
    pub nesting_depth: f64,
    pub suspicious_literal_count: f64,
    /// S4.2: mutable default args (`def f(x=[])`, `def f(x={})`).
    /// Python/JS classic LLM trap — the default object is shared
    /// across calls.
    pub mutable_default_arg_count: f64,
    /// S4.3: same-scope re-binding to a variable that already has
    /// a value. Captures `result = compute(); ...; result = result["data"]`
    /// (silent overwrite) but only when the RHS doesn't reference
    /// the LHS (those are legit accumulators).
    pub shadowed_local_count: f64,
    /// S4.1: number of test functions in this file. The intent is
    /// inverse-cost — when this *decreases* between old and new
    /// content, that's a regression. Flipped sign in
    /// `extract_signals_native` so the cost-regression layer fires
    /// on test removals (which would otherwise look like "code got
    /// shorter, congrats").
    pub test_count: f64,
}

/// Walk a file once and return all smell counters.
pub fn smell_counts(filepath: &str) -> Result<SmellCounts, String> {
    let code = std::fs::read_to_string(filepath).map_err(|e| e.to_string())?;
    let Some(adapter) = LanguageRegistry::global().for_path(filepath) else {
        return Ok(SmellCounts::default());
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(adapter.tree_sitter_language())
        .map_err(|e| e.to_string())?;
    let tree = parser.parse(&code, None).ok_or("parse returned None")?;
    Ok(scan(tree.root_node(), code.as_bytes()))
}

fn scan(root: Node, src: &[u8]) -> SmellCounts {
    let mut out = SmellCounts::default();
    walk(root, src, 0, &mut out);
    // unfinished_marker scan is on raw source (catches comments
    // tree-sitter sometimes hides under `comment` kind anyway, plus
    // we already cover marker tokens like todo!() through walk()).
    out.unfinished_marker_count += scan_text_markers(src);
    out
}

fn walk(node: Node, src: &[u8], depth: usize, out: &mut SmellCounts) {
    let kind = node.kind();
    if depth as f64 > out.nesting_depth && is_nesting_introducer(kind) {
        out.nesting_depth = depth as f64;
    }
    if is_branching_node(kind) {
        out.cyclomatic_complexity += 1.0;
    }
    if is_handler_clause(kind) && handler_body_is_empty(node, src) {
        out.empty_handler_count += 1.0;
    }
    if is_block_node(kind) {
        out.unreachable_stmt_count += count_unreachable_in_block(node);
        // S4.3: scope-local shadow detection runs on each block node.
        out.shadowed_local_count += count_shadowed_locals_in_block(node, src);
    }
    if is_string_literal(kind) {
        if let Ok(text) = node.utf8_text(src) {
            if is_suspicious_literal(text) {
                out.suspicious_literal_count += 1.0;
            }
        }
    }
    if is_marker_call(kind) {
        if let Ok(text) = node.utf8_text(src) {
            if MARKER_CALL_NAMES.iter().any(|n| text.starts_with(n)) {
                out.unfinished_marker_count += 1.0;
            }
        }
    }
    // S4.2: mutable default arg — check at the parameter level.
    if is_default_parameter(kind) && default_value_is_mutable(node, src) {
        out.mutable_default_arg_count += 1.0;
    }
    // S4.1: count test functions (named test_* or marked with test
    // decorators / wrapped in describe/it blocks).
    if is_test_function(node, src) {
        out.test_count += 1.0;
    }

    let next_depth = if is_nesting_introducer(kind) { depth + 1 } else { depth };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, next_depth, out);
    }
}

fn is_default_parameter(kind: &str) -> bool {
    matches!(
        kind,
        "default_parameter" | "typed_default_parameter"  // Python
            | "required_parameter" | "optional_parameter"  // TS — covered when has =
            | "formal_parameter" | "parameter"             // generic
    )
}

fn default_value_is_mutable(node: Node, src: &[u8]) -> bool {
    // Only flag when a default value is a literal mutable container.
    // The default is on the right of `=`. We grab field "value" or
    // the last named child; heuristic.
    let value = node.child_by_field_name("value").or_else(|| {
        let mut cursor = node.walk();
        let mut last = None;
        for c in node.named_children(&mut cursor) {
            last = Some(c);
        }
        last
    });
    let Some(value) = value else { return false };
    let kind = value.kind();
    if matches!(
        kind,
        "list" | "dictionary" | "set" | "array" | "object" | "array_pattern" | "object_pattern"
    ) {
        return true;
    }
    if let Ok(text) = value.utf8_text(src) {
        let trimmed = text.trim();
        if matches!(trimmed, "[]" | "{}" | "set()" | "list()" | "dict()" | "new Array()" | "new Object()" | "new Map()" | "new Set()") {
            return true;
        }
    }
    false
}

fn is_test_function(node: Node, src: &[u8]) -> bool {
    // Function definitions whose name starts with `test_` (Python /
    // Go convention) or `it("...")` / `test("...")` calls (JS test
    // runners). We only care about a count, not the kind, so very
    // permissive matching is fine — the cost-regression layer
    // catches it via *change* in count, not absolute value.
    let kind = node.kind();
    if matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_item"
            | "method_definition"
    ) {
        if let Some(name) = node.child_by_field_name("name") {
            if let Ok(n) = name.utf8_text(src) {
                if n.starts_with("test_") || n.starts_with("Test") {
                    return true;
                }
            }
        }
    }
    if matches!(kind, "call" | "call_expression") {
        if let Some(func) = node.child_by_field_name("function") {
            if let Ok(n) = func.utf8_text(src) {
                if matches!(n, "it" | "test" | "describe") {
                    return true;
                }
            }
        }
    }
    false
}

/// S4.3: count assignments inside a block where the LHS identifier
/// already had a binding earlier in the same block AND the RHS
/// does not reference the LHS (those are accumulators, e.g.,
/// `total = total + x` — legit).
fn count_shadowed_locals_in_block(node: Node, src: &[u8]) -> f64 {
    use std::collections::HashSet;
    let mut bound: HashSet<String> = HashSet::new();
    let mut shadows: f64 = 0.0;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        // Python: `x = expr` parses as expression_statement→assignment;
        // JS: lexical_declaration / variable_declaration / assignment_expression.
        let assign_node = if kind == "expression_statement" {
            child.named_child(0)
        } else {
            Some(child)
        };
        let Some(assign) = assign_node else { continue };
        if !matches!(
            assign.kind(),
            "assignment" | "augmented_assignment" | "assignment_expression"
        ) {
            continue;
        }
        let Some(lhs) = assign.child_by_field_name("left") else { continue };
        let Some(rhs) = assign.child_by_field_name("right") else { continue };
        let Ok(name) = lhs.utf8_text(src) else { continue };
        // Accumulator pattern — RHS contains LHS name → not a shadow.
        if let Ok(rhs_text) = rhs.utf8_text(src) {
            if rhs_text.contains(name) {
                bound.insert(name.to_string());
                continue;
            }
        }
        if bound.contains(name) {
            shadows += 1.0;
        } else {
            bound.insert(name.to_string());
        }
    }
    shadows
}

fn is_nesting_introducer(kind: &str) -> bool {
    // S1.4: deliberately excludes `block`/`compound_statement` —
    // those are *children* of every function/loop/if and would
    // double-count depth otherwise (e.g., a Rust function body
    // would be depth 2 immediately on entry: function_item → block).
    matches!(
        kind,
        "if_statement" | "if_expression"
        | "for_statement" | "for_in_statement" | "for_of_statement"
        | "while_statement" | "while_expression"
        | "do_statement" | "do_while_statement"
        | "loop_statement" | "loop_expression"
        | "try_statement" | "try_expression"
        | "match_expression" | "switch_statement" | "switch_expression"
        | "case_clause" | "case_block" | "match_arm"
        | "with_statement"
        | "function_definition" | "function_declaration" | "method_declaration"
        | "function_item" | "method_definition" | "arrow_function"
        | "lambda" | "closure_expression"
    )
}

fn is_branching_node(kind: &str) -> bool {
    // S1.5: deliberately excludes `else_clause`. Classic McCabe
    // counts `if/else` as 1 decision point (the predicate), not 2
    // (one for if and one for else). Same logic for `elif` — but
    // an `elif` introduces a new predicate so it does count.
    matches!(
        kind,
        "if_statement" | "elif_clause" | "elif_statement"
        | "for_statement" | "for_in_statement" | "for_of_statement"
        | "while_statement"
        | "do_statement" | "do_while_statement"
        | "loop_statement"
        | "case_clause" | "case_block" | "match_arm" | "switch_case"
        | "catch_clause" | "except_clause"
        | "ternary_expression" | "conditional_expression"
    )
}

fn is_handler_clause(kind: &str) -> bool {
    matches!(
        kind,
        "catch_clause" | "except_clause" | "catch_block" | "rescue_clause"
    )
}

fn handler_body_is_empty(node: Node, src: &[u8]) -> bool {
    // S1.9: be conservative about identifying the body. Try several
    // explicit field names first; only fall back to "last named child"
    // if it actually looks like a block (named children of certain
    // grammars include the exception variable as a sibling — picking
    // the last-child blindly would treat the variable as the body).
    let body = node
        .child_by_field_name("body")
        .or_else(|| node.child_by_field_name("block"))
        .or_else(|| node.child_by_field_name("handler"))
        .or_else(|| {
            // Find the last child that is itself a block-like node.
            let mut cursor = node.walk();
            let mut found: Option<Node> = None;
            for child in node.named_children(&mut cursor) {
                if matches!(
                    child.kind(),
                    "block" | "compound_statement" | "statement_block"
                        | "function_body" | "case_block" | "rescue_clause"
                ) {
                    found = Some(child);
                }
            }
            found
        });
    let Some(body) = body else { return false };
    let count = body.named_child_count();
    if count == 0 {
        return true;
    }
    if count == 1 {
        // Single statement — check if it's pass / continue / null
        // return / empty.
        if let Some(only) = body.named_child(0) {
            let k = only.kind();
            if matches!(
                k,
                "pass_statement" | "comment" | "empty_statement"
            ) {
                return true;
            }
            if let Ok(text) = only.utf8_text(src) {
                let trimmed = text.trim();
                if matches!(
                    trimmed,
                    "pass" | ";" | "" | "return" | "return null" | "return None" | "return false"
                ) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_block_node(kind: &str) -> bool {
    matches!(
        kind,
        "block" | "compound_statement" | "statement_block" | "function_body" | "case_block"
    )
}

/// Count statements that follow a definite terminator (return/throw/
/// break/continue) within a block, ignoring trailing comments.
fn count_unreachable_in_block(node: Node) -> f64 {
    let mut found_terminator = false;
    let mut count: f64 = 0.0;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if found_terminator && child.kind() != "comment" {
            count += 1.0;
        }
        if is_terminator_node(child.kind()) {
            found_terminator = true;
        }
    }
    count
}

fn is_terminator_node(kind: &str) -> bool {
    matches!(
        kind,
        "return_statement"
        | "throw_statement"
        | "raise_statement"
        | "break_statement"
        | "continue_statement"
    )
}

fn is_string_literal(kind: &str) -> bool {
    matches!(
        kind,
        "string" | "string_literal" | "interpreted_string_literal"
        | "raw_string_literal" | "string_fragment"
    )
}

fn is_suspicious_literal(text: &str) -> bool {
    let s = text.trim_matches(|c: char| c == '"' || c == '\'' || c == '`');
    if s.is_empty() {
        return false;
    }
    // Known secret prefixes.
    if s.starts_with("sk-")
        || s.starts_with("sk_live_") || s.starts_with("sk_test_")
        || s.starts_with("ghp_") || s.starts_with("gho_")
        || s.starts_with("xoxb-") || s.starts_with("xoxp-") || s.starts_with("xoxs-")
        || s.starts_with("Bearer ")
    {
        return true;
    }
    // AWS access key id.
    if s.len() == 20 && s.starts_with("AKIA") && s[4..].chars().all(|c| c.is_ascii_alphanumeric()) {
        return true;
    }
    // Google API key.
    if s.len() == 39 && s.starts_with("AIza") {
        return true;
    }
    // Hardcoded localhost with explicit port.
    if (s.starts_with("localhost:") || s.starts_with("127.0.0.1:") || s.starts_with("0.0.0.0:"))
        && s.bytes().filter(|b| b.is_ascii_digit()).count() >= 2
    {
        return true;
    }
    // Hardcoded user-home / Windows-drive paths (LLMs often leak
    // local paths from training).
    if s.starts_with("/home/") || s.starts_with("/Users/") {
        return true;
    }
    if s.len() >= 3 && s.as_bytes()[1] == b':' && (s.starts_with("C:\\") || s.starts_with("D:\\")) {
        return true;
    }
    false
}

fn is_marker_call(kind: &str) -> bool {
    // Rust-only specific call shapes; all other languages caught by
    // the text scan.
    matches!(kind, "macro_invocation" | "expression_statement")
}

const MARKER_CALL_NAMES: &[&str] = &[
    "todo!", "unimplemented!", "unreachable!",
];

const COMMENT_MARKER_RE: &[&str] = &[
    "TODO", "FIXME", "XXX", "HACK",
    "NotImplementedError",
];

fn scan_text_markers(src: &[u8]) -> f64 {
    let Ok(text) = std::str::from_utf8(src) else {
        return 0.0;
    };
    let mut count: f64 = 0.0;
    for needle in COMMENT_MARKER_RE {
        // Naive substring count is fine — these markers are rare
        // enough that false positives (e.g. "TODO" appearing in
        // user-facing copy) are nearly always still smells.
        let mut start = 0;
        while let Some(pos) = text[start..].find(needle) {
            count += 1.0;
            start += pos + needle.len();
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(suffix: &str, body: &str) -> tempfile::NamedTempFile {
        let mut tmp = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        tmp.write_all(body.as_bytes()).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn empty_except_python() {
        let tmp = write_tmp(".py", "try:\n    x = 1\nexcept Exception:\n    pass\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.empty_handler_count >= 1.0, "expected empty_handler>=1, got {c:?}");
    }

    #[test]
    fn empty_catch_typescript() {
        let tmp = write_tmp(".ts", "try { foo(); } catch (e) { }\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.empty_handler_count >= 1.0, "expected empty catch>=1, got {c:?}");
    }

    #[test]
    fn unfinished_markers_counted() {
        let tmp = write_tmp(".py", "# TODO: implement this\n# FIXME: race condition\nx = 1\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.unfinished_marker_count >= 2.0, "got {c:?}");
    }

    #[test]
    fn rust_todo_macro_counted() {
        let tmp = write_tmp(".rs", "fn f() { todo!() }\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.unfinished_marker_count >= 1.0, "got {c:?}");
    }

    #[test]
    fn unreachable_after_return() {
        let tmp = write_tmp(
            ".py",
            "def f():\n    return 1\n    x = 2\n    y = 3\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.unreachable_stmt_count >= 2.0, "got {c:?}");
    }

    #[test]
    fn cyclomatic_counts_branches() {
        let tmp = write_tmp(
            ".py",
            "def f(x):\n    if x:\n        pass\n    elif x == 1:\n        pass\n    for i in range(3):\n        pass\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.cyclomatic_complexity >= 3.0, "got {c:?}");
    }

    #[test]
    fn nesting_depth_tracks_pyramid() {
        let tmp = write_tmp(
            ".py",
            "def f():\n    if a:\n        if b:\n            if c:\n                if d:\n                    pass\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.nesting_depth >= 4.0, "got {c:?}");
    }

    #[test]
    fn suspicious_literal_secret_prefix() {
        let tmp = write_tmp(".py", "API_KEY = \"sk-abcd1234efgh5678ijkl\"\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.suspicious_literal_count >= 1.0, "got {c:?}");
    }

    #[test]
    fn suspicious_literal_localhost_with_port() {
        let tmp = write_tmp(".py", "URL = \"localhost:5432\"\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.suspicious_literal_count >= 1.0, "got {c:?}");
    }

    #[test]
    fn mutable_default_arg_python() {
        let tmp = write_tmp(".py", "def f(items=[]):\n    items.append(1)\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.mutable_default_arg_count >= 1.0, "got {c:?}");
    }

    #[test]
    fn shadowed_local_python() {
        let tmp = write_tmp(
            ".py",
            "def f():\n    result = compute()\n    log(result)\n    result = fetch()\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.shadowed_local_count >= 1.0, "got {c:?}");
    }

    #[test]
    fn accumulator_not_flagged_as_shadow() {
        let tmp = write_tmp(
            ".py",
            "def f(items):\n    total = 0\n    for x in items:\n        total = total + x\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(c.shadowed_local_count, 0.0, "accumulator should not flag, got {c:?}");
    }

    #[test]
    fn test_count_picks_up_test_functions() {
        let tmp = write_tmp(
            ".py",
            "def test_one():\n    assert True\n\ndef test_two():\n    assert True\n\ndef helper():\n    pass\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.test_count >= 2.0, "got {c:?}");
    }

    #[test]
    fn test_count_picks_up_jest_blocks() {
        let tmp = write_tmp(
            ".ts",
            "describe('x', () => {\n  it('does y', () => {});\n  it('does z', () => {});\n});\n",
        );
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert!(c.test_count >= 2.0, "got {c:?}");
    }

    #[test]
    fn clean_file_has_zero_smells() {
        let tmp = write_tmp(".py", "def add(a, b):\n    return a + b\n");
        let c = smell_counts(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(c.empty_handler_count, 0.0);
        assert_eq!(c.unfinished_marker_count, 0.0);
        assert_eq!(c.unreachable_stmt_count, 0.0);
        assert_eq!(c.suspicious_literal_count, 0.0);
    }
}
