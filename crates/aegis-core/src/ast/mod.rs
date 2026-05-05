pub mod adapter;
pub mod languages;
pub mod parsed_file;
pub mod parser;
pub mod registry;

pub use adapter::{default_max_chain_depth, LanguageAdapter};
pub use parsed_file::{parse, ParsedFile};
pub use parser::get_imports_native;
pub use registry::LanguageRegistry;
