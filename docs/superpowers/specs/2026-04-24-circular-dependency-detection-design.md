# Circular Dependency Detection — Design Spec

**Date:** 2026-04-24  
**Scope:** Wire `DependencyGraph` (Ring 0 Rust core) into the `aegis check` CLI command

---

## Problem

`DependencyGraph` is fully implemented in Rust (`graph_engine.rs`) and exported to Python, but the `check` command in `cli.py` has a `# TODO: Build Graph for Circular Dependency` placeholder at line 41. The `anti_circular_dependency` rule in `default_core_policy.yaml` is never evaluated.

---

## Approach: A1 — Rust extracts imports, Python resolves paths

Rust handles syntax-layer extraction. Python handles filesystem-layer path resolution. This keeps Ring 0's responsibility boundary clean and allows future TS/Go support by adding language-specific `get_imports` variants in Rust only.

---

## Design

### Part 1 — Rust (`ast_parser.rs`)

Add a new PyO3 function:

```rust
#[pyfunction]
pub fn get_imports(filepath: &str) -> PyResult<Vec<String>>
```

- Reuses the existing tree-sitter query (`queries/python.scm`)
- Returns a deduplicated list of imported module name strings
  - e.g. `["os", "sys", "mymodule", "pkg.sub"]`
- Does not perform path resolution (syntax layer only)

Export in `lib.rs`:
```rust
m.add_function(wrap_pyfunction!(ast_parser::get_imports, m)?)?;
```

Rebuild with `maturin develop` after the change.

### Part 2 — Python (`cli.py`, `check` command)

Replace the `# TODO` block with the following logic:

1. Read `anti_circular_dependency.enabled` from the policy YAML. If `false`, skip the entire graph phase.
2. Build `module_map: dict[str, str]` mapping module names to file paths for all `.py` files found in the project:
   - `import foo` → `<root>/foo.py` or `<root>/foo/__init__.py`
   - `from foo.bar import x` → `<root>/foo/bar.py` or `<root>/foo/bar/__init__.py`
   - Files not found in the project directory are treated as external packages and skipped.
3. For each file, call `aegis_core_rs.get_imports(filepath)`, filter to project-internal imports using `module_map`, and collect `(source_file, target_file)` edges.
4. Call `DependencyGraph.build_from_edges(edges)`.
5. Call `dg.check_circular_dependency()`. If `True`, read `anti_circular_dependency.message` from the policy YAML and print it, then set `has_violations = True`.

### Part 3 — Error Handling & Output

**Output format (consistent with existing per-file violations):**
```
Circular dependency detected across project files.
  <anti_circular_dependency.message from policy YAML>
Aegis check failed.
```

**Edge cases:**
- `get_imports()` raises → caught by existing `try/except`, prints a warning and continues
- Only one `.py` file in project → graph can have no cycle, skip `check_circular_dependency()`
- `anti_circular_dependency.enabled: false` → skip graph build entirely

**Out of scope for this change:**
- Reporting which specific modules form the cycle (future enhancement)
- Modifying `LLMGateway.validate()` — it operates per-file and circular dependency is a project-level concern

---

## Files Changed

| File | Change |
|------|--------|
| `aegis-core-rs/src/ast_parser.rs` | Add `get_imports()` function |
| `aegis-core-rs/src/lib.rs` | Export `get_imports` |
| `aegis/cli.py` | Replace TODO with graph-based circular dependency check |

---

## Success Criteria

- `aegis check` on a project with a circular import (`A → B → A`) exits with code 1 and prints the policy message
- `aegis check` on a clean project still exits 0 with no false positives
- `get_imports()` is callable from Python and returns the correct import strings
