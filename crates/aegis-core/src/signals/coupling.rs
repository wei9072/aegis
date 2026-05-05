//! Fan-out signal — count distinct external imports from a parsed file.
//!
//! Language-agnostic via the registry. Files with no registered
//! adapter never reach this layer (parse() returns None earlier).

use tree_sitter::{Query, QueryCursor};

use crate::ast::parsed_file::ParsedFile;

/// Count unique import targets (fan-out) from a pre-parsed file.
/// Returns 0 when the adapter's import query fails to compile
/// (defensive — the registry meta-test guards this at startup).
pub fn fan_out(parsed: &ParsedFile<'_>) -> usize {
    let adapter = parsed.adapter();
    let lang = adapter.tree_sitter_language();
    let query = match Query::new(lang, adapter.import_query()) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    let mut qc = QueryCursor::new();
    let mut seen = std::collections::HashSet::new();
    let src = parsed.source_bytes();
    for m in qc.matches(&query, parsed.root_node(), src) {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(src) {
                seen.insert(adapter.normalize_import(text));
            }
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::parsed_file::parse;

    #[test]
    fn python_relative_import_is_counted() {
        let pf = parse("a.py", "from . import foo\nfrom .bar import baz\nimport os\n")
            .expect("python parses");
        let n = fan_out(&pf);
        assert!(n >= 2, "expected relative imports + os; got fan_out={n}");
    }

    #[test]
    fn typescript_export_from_is_counted() {
        let pf = parse("a.ts", "export { a } from './a';\nexport * from './b';\n")
            .expect("ts parses");
        let n = fan_out(&pf);
        assert!(n >= 2, "expected ./a + ./b; got fan_out={n}");
    }

    #[test]
    fn typescript_dynamic_import_is_counted() {
        let pf = parse("a.ts", "const m = import('./dyn');\n").expect("ts parses");
        let n = fan_out(&pf);
        assert!(n >= 1, "expected dynamic import('./dyn'); got fan_out={n}");
    }

    #[test]
    fn javascript_export_from_and_dynamic_import_counted() {
        let pf = parse("a.js", "export { a } from './a';\nimport('./b');\n")
            .expect("js parses");
        let n = fan_out(&pf);
        assert!(n >= 2, "expected ./a + ./b; got fan_out={n}");
    }
}
