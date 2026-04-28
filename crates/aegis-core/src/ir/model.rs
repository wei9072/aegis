#[derive(Debug, Clone, PartialEq)]
pub enum IrNodeKind {
    Dependency,
}

#[derive(Debug, Clone)]
pub struct IrNode {
    pub kind: String,
    pub file_path: String,
    pub name: String,
}

impl IrNode {
    pub fn new(kind: String, file_path: String, name: String) -> Self {
        IrNode { kind, file_path, name }
    }
}
