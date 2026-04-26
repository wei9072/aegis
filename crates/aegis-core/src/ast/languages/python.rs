use tree_sitter::Language;

pub fn language() -> Language {
    tree_sitter_python::language()
}

pub const IMPORT_QUERY: &str = include_str!("../../../queries/python.scm");
