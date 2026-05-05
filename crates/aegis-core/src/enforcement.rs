//! Syntax-violation extraction. Walks a pre-parsed tree looking for
//! ERROR / MISSING nodes and returns located violations with 1-based
//! line/col anchors.
//!
//! V2 stops short-circuiting on syntax errors — emitting these as
//! findings lets the consuming agent decide whether they matter
//! given the change context. Tree-sitter is error-tolerant by
//! design; downstream signal/security walkers cope with degraded
//! trees by simply not matching where the AST is corrupted.

use crate::ast::parsed_file::ParsedFile;

/// Located syntax violation: where a bad node sits in the source.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyntaxViolation {
    pub message: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub kind: String, // "error" | "missing"
}

/// Extract syntax violations from a pre-parsed file. Returns the
/// first 5 ERROR/MISSING anchors (cap to keep findings compact —
/// agents only need anchors, not the full failure list). Returns
/// empty when the parse is clean.
///
/// `filepath` is used only for the human-readable message text — no
/// IO happens.
pub fn syntax_violations(parsed: &ParsedFile<'_>, filepath: &str) -> Vec<SyntaxViolation> {
    let root = parsed.root_node();
    if !root.has_error() {
        return vec![];
    }

    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if out.len() >= 5 {
            break;
        }
        if node.is_error() || node.is_missing() {
            let s = node.start_position();
            let e = node.end_position();
            out.push(SyntaxViolation {
                message: format!(
                    "[Syntax] {} node at {}:{}–{}:{} in '{}'",
                    if node.is_missing() { "MISSING" } else { "ERROR" },
                    s.row + 1,
                    s.column + 1,
                    e.row + 1,
                    e.column + 1,
                    filepath,
                ),
                start_line: s.row + 1,
                start_col: s.column + 1,
                end_line: e.row + 1,
                end_col: e.column + 1,
                kind: if node.is_missing() {
                    "missing".into()
                } else {
                    "error".into()
                },
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    if out.is_empty() {
        // root.has_error() but no specific node found — fall back to
        // a generic violation pointing at the file.
        out.push(SyntaxViolation {
            message: format!(
                "[Syntax] Syntax error detected in '{}'. Fix syntax before proceeding.",
                filepath
            ),
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 1,
            kind: "error".into(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::parsed_file::parse;

    #[test]
    fn clean_python_yields_no_violations() {
        let pf = parse("a.py", "def hello():\n    return 42\n").unwrap();
        assert!(syntax_violations(&pf, "a.py").is_empty());
    }

    #[test]
    fn broken_python_yields_violation() {
        let pf = parse("a.py", "def err(\n").unwrap();
        let v = syntax_violations(&pf, "a.py");
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("[Syntax]"));
    }

    #[test]
    fn high_fan_out_is_not_a_syntax_error() {
        let body: String = (0..20).map(|i| format!("import mod_{}\n", i)).collect();
        let pf = parse("a.py", &body).unwrap();
        assert!(syntax_violations(&pf, "a.py").is_empty());
    }
}
