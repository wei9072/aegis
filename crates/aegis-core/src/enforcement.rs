//! Ring 0 syntax-validity check — language-agnostic via the
//! registry. Returns one violation string per failing file with the
//! same shape V0.x callers expect.

use std::fs;

use crate::ast::registry::LanguageRegistry;

/// Located syntax violation: where the bad node is.
/// Populated by `check_syntax_native_detailed`; consumed by
/// `validate_change` to enrich MCP responses so upstream agents can
/// jump to the actual line instead of bisecting blindly.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyntaxViolation {
    pub message: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub kind: String, // "error" | "missing"
}

/// Pure-Rust Ring 0 syntax check. Returns
/// `Result<Vec<String>, String>`; non-empty means the file failed
/// to parse cleanly.
pub fn check_syntax_native(filepath: &str) -> Result<Vec<String>, String> {
    Ok(check_syntax_native_detailed(filepath)?
        .into_iter()
        .map(|v| v.message)
        .collect())
}

/// Same Ring 0 check but returns located violations with line/col
/// ranges and node kind. Used by MCP enrichment.
pub fn check_syntax_native_detailed(filepath: &str) -> Result<Vec<SyntaxViolation>, String> {
    let code = fs::read_to_string(filepath).map_err(|e| e.to_string())?;

    let registry = LanguageRegistry::global();
    let adapter = match registry.for_path(filepath) {
        Some(a) => a,
        None => return Ok(vec![]),
    };

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(adapter.tree_sitter_language())
        .map_err(|e| e.to_string())?;
    let tree = parser
        .parse(&code, None)
        .ok_or_else(|| "parse returned None".to_string())?;

    let root = tree.root_node();
    if !root.has_error() {
        return Ok(vec![]);
    }

    // Walk to find ERROR / MISSING nodes. Cap at first 5 to keep
    // the verdict compact; the agent only needs anchors, not the
    // full failure list.
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
                    "[Ring 0] {} node at {}:{}–{}:{} in '{}'",
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
                kind: if node.is_missing() { "missing".into() } else { "error".into() },
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
                "[Ring 0] Syntax error detected in '{}'. Fix syntax before proceeding.",
                filepath
            ),
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 1,
            kind: "error".into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_with(suffix: &str, body: &[u8]) -> tempfile::NamedTempFile {
        let mut tmp = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        tmp.write_all(body).unwrap();
        tmp.flush().unwrap();
        tmp
    }

    #[test]
    fn test_valid_python_file() {
        let tmp = tmp_with(".py", b"def hello():\n    return 42\n");
        assert!(check_syntax_native(tmp.path().to_str().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn test_invalid_python_file() {
        let tmp = tmp_with(".py", b"def err(\n");
        let v = check_syntax_native(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("[Ring 0]"));
    }

    #[test]
    fn test_unknown_extension_returns_no_violations() {
        let tmp = tmp_with(".whatever", b"this is not parseable code");
        assert!(check_syntax_native(tmp.path().to_str().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn test_high_fan_out_not_blocked() {
        let body: Vec<u8> = (0..20)
            .flat_map(|i| format!("import mod_{}\n", i).into_bytes())
            .collect();
        let tmp = tmp_with(".py", &body);
        assert!(check_syntax_native(tmp.path().to_str().unwrap()).unwrap().is_empty());
    }
}
