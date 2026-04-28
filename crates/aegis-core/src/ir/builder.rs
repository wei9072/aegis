use crate::ast::parser::get_imports_native;
use crate::ir::model::IrNode;

/// Build a flat IR from a single file: one IrNode per import dependency.
pub fn build_ir(filepath: &str) -> Result<Vec<IrNode>, String> {
    let imports = get_imports_native(filepath)?;
    Ok(imports
        .into_iter()
        .map(|name| IrNode::new("dependency".to_string(), filepath.to_string(), name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_build_ir_returns_dependency_nodes() {
        let code = b"import os\nimport sys\n";
        let mut tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let nodes = build_ir(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.kind == "dependency"));
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"os"));
        assert!(names.contains(&"sys"));
    }

    #[test]
    fn test_build_ir_empty_file() {
        let code = b"x = 1\n";
        let mut tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
        tmp.write_all(code).unwrap();
        tmp.flush().unwrap();
        let nodes = build_ir(tmp.path().to_str().unwrap()).unwrap();
        assert!(nodes.is_empty());
    }
}
