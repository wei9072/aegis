use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum IrNodeKind {
    Dependency,
}

#[pyclass(get_all)]
#[derive(Debug, Clone)]
pub struct IrNode {
    pub kind: String,
    pub file_path: String,
    pub name: String,
}

#[pymethods]
impl IrNode {
    #[new]
    pub fn new(kind: String, file_path: String, name: String) -> Self {
        IrNode { kind, file_path, name }
    }

    pub fn __repr__(&self) -> String {
        format!("IrNode({} '{}' in {})", self.kind, self.name, self.file_path)
    }
}
