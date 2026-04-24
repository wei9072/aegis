use pyo3::prelude::*;
use std::fs;
use crate::ast::languages::python;

#[pyfunction]
pub fn check_syntax(filepath: &str) -> PyResult<Vec<String>> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(python::language())
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    let tree = parser.parse(&code, None)
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("parse returned None"))?;

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

    #[test]
    fn test_valid_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"def hello():\n    return 42\n").unwrap();
        assert!(check_syntax(tmp.path().to_str().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn test_invalid_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"def err(\n").unwrap();
        let v = check_syntax(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("[Ring 0]"));
    }

    #[test]
    fn test_high_fan_out_not_blocked() {
        let imports: Vec<u8> = (0..20)
            .flat_map(|i| format!("import mod_{}\n", i).into_bytes())
            .collect();
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(&imports).unwrap();
        assert!(check_syntax(tmp.path().to_str().unwrap()).unwrap().is_empty());
    }
}
