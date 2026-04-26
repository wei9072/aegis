pub mod adapter;
pub mod languages;
pub mod parser;
pub mod registry;

pub use adapter::{default_max_chain_depth, LanguageAdapter};
pub use parser::{analyze_file, get_imports, AstMetrics};
pub use registry::LanguageRegistry;
