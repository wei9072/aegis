//! `unresolved_local_import_count` — relative imports that point at
//! files that do not exist on disk.
//!
//! Targets a specific LLM failure mode: hallucinated module names.
//! When refactoring, models often invent plausible-looking imports
//! like `from .helpers import process` even when `helpers.py`
//! doesn't exist in the package — Ring 0 syntax passes, but the
//! file won't import at runtime.
//!
//! Scope discipline: we only check **relative** imports (`./foo`,
//! `from . import bar`, `crate::`, `super::`). External packages
//! (`numpy`, `react`, `serde`) are out of scope — checking those
//! would require resolving package roots / lock files, which
//! violates the "single fast pass" rule.

use std::path::{Path, PathBuf};

use tree_sitter::{Query, QueryCursor};

use crate::ast::registry::LanguageRegistry;

/// Count relative imports in `code` that don't resolve to an
/// existing sibling file. `file_path` is the file the imports live
/// in; its parent directory is the resolution root.
pub fn unresolved_local_import_count(file_path: &str, code: &str) -> f64 {
    let registry = LanguageRegistry::global();
    let Some(adapter) = registry.for_path(file_path) else {
        return 0.0;
    };
    let parent = Path::new(file_path).parent().map(PathBuf::from);
    let Some(parent) = parent else { return 0.0 };

    let lang = adapter.tree_sitter_language();
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() {
        return 0.0;
    }
    let Some(tree) = parser.parse(code, None) else {
        return 0.0;
    };
    let Ok(query) = Query::new(lang, adapter.import_query()) else {
        return 0.0;
    };
    let mut qc = QueryCursor::new();
    let mut unresolved: f64 = 0.0;
    let extensions = adapter.extensions();
    for m in qc.matches(&query, tree.root_node(), code.as_bytes()) {
        for cap in m.captures {
            let Ok(raw) = cap.node.utf8_text(code.as_bytes()) else {
                continue;
            };
            let cleaned = adapter.normalize_import(raw);
            if !is_relative_import(&cleaned) {
                continue;
            }
            if !import_resolves(&parent, &cleaned, extensions) {
                unresolved += 1.0;
            }
        }
    }
    // S1.6: Python `from . import name` doesn't capture `name` via the
    // import_query (only the relative_import module is captured). So
    // we additionally walk the AST for `import_from_statement` whose
    // module_name is bare `.` and check whether the imported names
    // resolve to sibling files.
    if file_path.ends_with(".py") || file_path.ends_with(".pyi") {
        unresolved += scan_python_bare_dot_imports(tree.root_node(), code.as_bytes(), &parent, extensions);
    }
    unresolved
}

fn scan_python_bare_dot_imports(
    node: tree_sitter::Node,
    src: &[u8],
    parent: &Path,
    extensions: &[&str],
) -> f64 {
    let mut total = 0.0;
    if node.kind() == "import_from_statement" {
        // Check whether module_name is the bare `.` form.
        let module_is_bare_dot = node
            .child_by_field_name("module_name")
            .and_then(|m| m.utf8_text(src).ok())
            .map(|s| s.trim() == ".")
            .unwrap_or(false);
        if module_is_bare_dot {
            // Iterate the `name:` fields — these are the imported names.
            // Use TreeCursor.field_name() rather than Node API so we
            // get the field per child reliably across grammar versions.
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let field = cursor.field_name();
                    if field == Some("name") {
                        let child = cursor.node();
                        let raw = child.utf8_text(src).unwrap_or("");
                        let name = match child.kind() {
                            "aliased_import" => child
                                .named_child(0)
                                .and_then(|n| n.utf8_text(src).ok())
                                .unwrap_or(raw),
                            _ => raw,
                        };
                        let name = name.trim();
                        if !name.is_empty() {
                            let candidate = parent.join(name);
                            let mut resolved = false;
                            for ext in extensions {
                                let with_ext = candidate.with_extension(ext.trim_start_matches('.'));
                                if with_ext.exists() {
                                    resolved = true;
                                    break;
                                }
                            }
                            if !resolved
                                && candidate.is_dir()
                                && candidate.join("__init__.py").exists()
                            {
                                resolved = true;
                            }
                            if !resolved {
                                total += 1.0;
                            }
                        }
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        total += scan_python_bare_dot_imports(child, src, parent, extensions);
    }
    total
}

fn is_relative_import(s: &str) -> bool {
    s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('.')   // Python `from . import` / `from .x`
        || s.starts_with("crate::")
        || s.starts_with("super::")
        || s.starts_with("self::")
}

fn import_resolves(parent: &Path, raw: &str, extensions: &[&str]) -> bool {
    // Normalize Python dotted-relative form `.foo.bar` → `foo/bar`,
    // strip leading dots into `../` for each extra dot.
    let candidate_paths = candidate_paths(parent, raw);
    for candidate in candidate_paths {
        for ext in extensions {
            let with_ext = candidate.with_extension(ext.trim_start_matches('.'));
            if with_ext.exists() {
                return true;
            }
        }
        // Directory with index file (e.g., TS `./foo` → `./foo/index.ts`)
        if candidate.is_dir() {
            for ext in extensions {
                let bare = ext.trim_start_matches('.');
                let idx = candidate.join(format!("index.{bare}"));
                let init = candidate.join("__init__.py");
                let mod_rs = candidate.join("mod.rs");
                if idx.exists() || init.exists() || mod_rs.exists() {
                    return true;
                }
            }
        }
        if candidate.exists() {
            return true;
        }
    }
    false
}

fn candidate_paths(parent: &Path, raw: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();

    if raw.starts_with("./") || raw.starts_with("../") {
        out.push(parent.join(raw));
        return out;
    }

    if raw.starts_with("crate::") {
        // Walk up to find a Cargo.toml as crate root, then resolve
        // the path beneath src/.
        if let Some(root) = find_crate_root(parent) {
            let path_part = raw.trim_start_matches("crate::").replace("::", "/");
            out.push(root.join("src").join(path_part));
        }
        return out;
    }

    if raw.starts_with("super::") || raw.starts_with("self::") {
        let mut path = parent.to_path_buf();
        let mut tail = raw.to_string();
        while tail.starts_with("super::") {
            path = path.parent().map(PathBuf::from).unwrap_or(path);
            tail = tail.trim_start_matches("super::").to_string();
        }
        tail = tail.trim_start_matches("self::").to_string();
        out.push(path.join(tail.replace("::", "/")));
        return out;
    }

    if raw.starts_with('.') {
        // Python relative import. Each leading dot beyond the first
        // = one parent step. `.foo.bar` → ./foo/bar. `..foo` → ../foo.
        let leading_dots = raw.chars().take_while(|c| *c == '.').count();
        let rest = &raw[leading_dots..];
        let mut path = parent.to_path_buf();
        for _ in 1..leading_dots {
            path = path.parent().map(PathBuf::from).unwrap_or(path);
        }
        if rest.is_empty() {
            out.push(path);
        } else {
            out.push(path.join(rest.replace('.', "/")));
        }
        return out;
    }

    out
}

fn find_crate_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        if dir.join("Cargo.toml").exists() {
            return Some(dir);
        }
        cur = dir.parent().map(PathBuf::from);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_python_unresolved_relative_import() {
        // The LLM-typical hallucination shape: a relative submodule
        // path that doesn't exist (e.g. `from .helpers import x` when
        // the model invented the helpers module).
        let dir = tempfile::tempdir().unwrap();
        let main_py = dir.path().join("main.py");
        std::fs::write(&main_py, "from .ghost_module import x\n").unwrap();
        let code = std::fs::read_to_string(&main_py).unwrap();
        let n = unresolved_local_import_count(main_py.to_str().unwrap(), &code);
        assert!(n >= 1.0, "expected unresolved>=1, got {n}");
    }

    #[test]
    fn passes_python_resolved_relative_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helpers.py"), "x = 1\n").unwrap();
        let main_py = dir.path().join("main.py");
        std::fs::write(&main_py, "from .helpers import x\n").unwrap();
        let code = std::fs::read_to_string(&main_py).unwrap();
        let n = unresolved_local_import_count(main_py.to_str().unwrap(), &code);
        assert_eq!(n, 0.0, "resolved import should not count, got {n}");
    }

    #[test]
    fn detects_typescript_unresolved_relative_import() {
        let dir = tempfile::tempdir().unwrap();
        let main_ts = dir.path().join("main.ts");
        std::fs::write(&main_ts, "import { x } from './ghost';\n").unwrap();
        let code = std::fs::read_to_string(&main_ts).unwrap();
        let n = unresolved_local_import_count(main_ts.to_str().unwrap(), &code);
        assert!(n >= 1.0, "expected unresolved>=1, got {n}");
    }

    #[test]
    fn passes_typescript_resolved_relative_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.ts"), "export const x = 1;\n").unwrap();
        let main_ts = dir.path().join("main.ts");
        std::fs::write(&main_ts, "import { x } from './real';\n").unwrap();
        let code = std::fs::read_to_string(&main_ts).unwrap();
        let n = unresolved_local_import_count(main_ts.to_str().unwrap(), &code);
        assert_eq!(n, 0.0, "resolved should not count, got {n}");
    }

    #[test]
    fn detects_python_bare_dot_unresolved_import() {
        // S1.6: `from . import ghost` previously slipped through
        // because `.` resolves to the parent dir which always exists.
        let dir = tempfile::tempdir().unwrap();
        let main_py = dir.path().join("main.py");
        std::fs::write(&main_py, "from . import ghost_sibling\n").unwrap();
        let code = std::fs::read_to_string(&main_py).unwrap();
        let n = unresolved_local_import_count(main_py.to_str().unwrap(), &code);
        assert!(n >= 1.0, "expected unresolved>=1, got {n}");
    }

    #[test]
    fn passes_python_bare_dot_resolved_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real_sibling.py"), "x = 1\n").unwrap();
        let main_py = dir.path().join("main.py");
        std::fs::write(&main_py, "from . import real_sibling\n").unwrap();
        let code = std::fs::read_to_string(&main_py).unwrap();
        let n = unresolved_local_import_count(main_py.to_str().unwrap(), &code);
        assert_eq!(n, 0.0, "resolved bare-dot import should not count, got {n}");
    }

    #[test]
    fn external_imports_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let main_py = dir.path().join("main.py");
        std::fs::write(&main_py, "import numpy\nimport tensorflow\n").unwrap();
        let code = std::fs::read_to_string(&main_py).unwrap();
        let n = unresolved_local_import_count(main_py.to_str().unwrap(), &code);
        assert_eq!(n, 0.0, "external imports out of scope, got {n}");
    }
}
