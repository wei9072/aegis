//! Ring 0 syntax-validity check — language-agnostic via the
//! registry. Returns one violation string per failing file with the
//! same shape V0.x callers expect.

use std::fs;

use crate::ast::registry::LanguageRegistry;

/// Pure-Rust Ring 0 syntax check. Returns
/// `Result<Vec<String>, String>`; non-empty means the file failed
/// to parse cleanly.
pub fn check_syntax_native(filepath: &str) -> Result<Vec<String>, String> {
    let code = fs::read_to_string(filepath).map_err(|e| e.to_string())?;

    let registry = LanguageRegistry::global();
    let adapter = match registry.for_path(filepath) {
        Some(a) => a,
        None => {
            // Unknown extension: no opinion. Higher layers (CLI,
            // pre-commit hook) decide whether to skip silently or
            // raise — Ring 0 just says "no syntax issues we can
            // see" and lets the caller act on the unsupported case.
            return Ok(vec![]);
        }
    };

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(adapter.tree_sitter_language())
        .map_err(|e| e.to_string())?;
    let tree = parser
        .parse(&code, None)
        .ok_or_else(|| "parse returned None".to_string())?;

    if tree.root_node().has_error() {
        Ok(vec![format!(
            "[Ring 0] Syntax error detected in '{}'. Fix syntax before proceeding.",
            filepath
        )])
    } else {
        Ok(vec![])
    }
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
