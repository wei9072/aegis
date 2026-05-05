//! Ring R2 — workspace structure layer.
//!
//! Cross-file checks that single-file `validate_change` cannot do.
//! LLMs frequently improve fan_out / chain_depth in one file while
//! breaking another file's imports — Ring R2 catches that.
//!
//! Current scope (V1):
//! - `cycle_introduced`: would the change create a new module
//!   import cycle?
//! - `public_symbols_lost`: did the change delete public symbols
//!   that other files depend on?
//! - `cross_file_unresolved_count`: how many files in the workspace
//!   now have unresolved relative imports?
//!
//! Future scope (V2): symbol-level public API surface diff (signature
//! changes, parameter list changes), incremental index with file
//! watchers, deeper symbol resolution.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use aegis_index::{InMemoryStore, IndexStore};
use tree_sitter::{Parser, Query, QueryCursor};

use crate::ast::parsed_file::{parse as parse_file, ParsedFile};
use crate::ast::registry::LanguageRegistry;
use crate::signals::{smell_counts_from_parsed, unresolved_local_import_count};

/// Per-file structural snapshot at a point in time.
#[derive(Debug, Clone, Default)]
pub struct FileSummary {
    pub imports: HashSet<String>,
    pub public_symbols: HashSet<String>,
    /// Named symbols imported from elsewhere (`from X import Y, Z`
    /// → {Y, Z}; `import { y, z } from 'X'` → {y, z}). Used by Ring
    /// R2 to detect broken-reference deletes. Best-effort across
    /// languages.
    pub imported_symbols: HashSet<String>,
    /// S7.7 — per-file numeric signals (cyclomatic, chain_depth,
    /// member_access_count, import_usage_count, type_leakage_count,
    /// cross_module_chain_count, …). Populated by summarize_file so
    /// project-wide z-score can be computed for any of them.
    pub signals: HashMap<String, f64>,
}

/// Eagerly-built workspace index. One pass over every supported
/// source file under `root`.
#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    pub root: PathBuf,
    pub files: HashMap<PathBuf, FileSummary>,
}

impl WorkspaceIndex {
    /// Build from disk. Walks `root` recursively (skipping hidden
    /// directories and common vendoring locations beneath root, but
    /// always entering the root itself even if its name happens to
    /// start with `.`, e.g. tempdir paths like `/tmp/.tmpXXXXXX`).
    ///
    /// Cold path. For interactive / hot paths use `build_cached`.
    pub fn build(root: &Path) -> Self {
        let mut idx = WorkspaceIndex {
            root: root.to_path_buf(),
            files: HashMap::new(),
        };
        let _ = walk_dir_inner(root, &mut idx.files, true);
        idx
    }

    /// S5 hot path: reuse a process-global mtime-keyed cache so that
    /// repeated calls (PreToolUse hook firing on every Edit) don't
    /// re-parse files that haven't changed. Falls back to `build`
    /// behaviour on the very first call.
    pub fn build_cached(root: &Path) -> Self {
        let store = global_store(root);
        let _ = aegis_index::refresh(
            root,
            store.as_ref(),
            |p| LanguageRegistry::global().for_path(&p.to_string_lossy()).is_some(),
            |path, code| summarize_file(path, code),
        );
        WorkspaceIndex {
            root: root.to_path_buf(),
            files: store.iter_summaries().into_iter().collect(),
        }
    }

    /// Apply a hypothetical change: replace (or insert) `path` with
    /// the parsed summary of `new_content`. Returns a fresh index
    /// (cheap — only the one entry differs).
    pub fn with_change(&self, path: &Path, new_content: &str) -> Self {
        let mut out = WorkspaceIndex {
            root: self.root.clone(),
            files: self.files.clone(),
        };
        let summary = summarize_file(path, new_content);
        out.files.insert(path.to_path_buf(), summary);
        out
    }

    /// V2 PR 3: Layer 1-shared variant of `with_change` — reuse a
    /// pre-parsed tree instead of re-parsing `new_content`. Use this
    /// when the caller already holds a `ParsedFile` (e.g., the
    /// validate path that just produced one for Ring 0/0.5/0.7).
    pub fn with_change_from_parsed(&self, path: &Path, parsed: &ParsedFile<'_>) -> Self {
        let mut out = WorkspaceIndex {
            root: self.root.clone(),
            files: self.files.clone(),
        };
        let summary = summarize_file_from_parsed(path, parsed);
        out.files.insert(path.to_path_buf(), summary);
        out
    }

    /// Number of files in the workspace that import `target_path`.
    pub fn fan_in(&self, target_path: &Path) -> usize {
        let normalized_targets = relative_import_targets(&self.root, target_path);
        self.files
            .iter()
            .filter(|(p, _)| p.as_path() != target_path)
            .filter(|(_, s)| {
                s.imports
                    .iter()
                    .any(|imp| normalized_targets.contains(imp))
            })
            .count()
    }

    /// fan_out of `path` from this index (count of unique imports
    /// it makes). Returns 0 if the path is unknown to the index —
    /// matches `fan_in`'s convention.
    pub fn fan_out(&self, path: &Path) -> usize {
        self.files
            .get(path)
            .map(|s| s.imports.len())
            .unwrap_or(0)
    }

    /// S7.2 — Robert Martin's instability metric:
    ///     I = fan_out / (fan_in + fan_out)
    /// Range [0.0, 1.0]:
    ///   - 0.0  → "stable": many depend on me, I depend on nobody.
    ///           Should be abstract (interface / contract). Changing
    ///           this file ripples through the workspace.
    ///   - 1.0  → "unstable": I depend on many, nobody depends on me.
    ///           Probably an entry point / leaf consumer. Changing
    ///           this file is local to itself.
    /// Returns None when both fan_in and fan_out are zero (file has
    /// no relationships either direction — I is undefined).
    pub fn instability(&self, path: &Path) -> Option<f64> {
        let fan_in = self.fan_in(path) as f64;
        let fan_out = self.fan_out(path) as f64;
        let total = fan_in + fan_out;
        if total == 0.0 {
            None
        } else {
            Some(fan_out / total)
        }
    }

    /// S7.3 — project-wide statistics for fan_out / fan_in. Used by
    /// callers to compute z-scores and tell whether one file's
    /// numbers are unusual for THIS project. fan_out=20 is normal in
    /// a Java enterprise codebase, anomalous in a small Python tool.
    /// Returns `(median, std_dev, count)` or None if the workspace
    /// has fewer than 2 files (std is undefined).
    pub fn fan_out_stats(&self) -> Option<(f64, f64, usize)> {
        let values: Vec<f64> = self
            .files
            .values()
            .map(|s| s.imports.len() as f64)
            .collect();
        descriptive_stats(&values)
    }

    pub fn fan_in_stats(&self) -> Option<(f64, f64, usize)> {
        let values: Vec<f64> = self
            .files
            .keys()
            .map(|p| self.fan_in(p) as f64)
            .collect();
        descriptive_stats(&values)
    }

    /// Z-score for `path`'s fan_out against the workspace median.
    /// Returns None when stats are unavailable (< 2 files) or std is
    /// zero (every file has the same fan_out — z-score undefined).
    pub fn fan_out_z_score(&self, path: &Path) -> Option<f64> {
        let (median, std, _) = self.fan_out_stats()?;
        if std == 0.0 {
            return None;
        }
        let v = self.fan_out(path) as f64;
        Some((v - median) / std)
    }

    pub fn fan_in_z_score(&self, path: &Path) -> Option<f64> {
        let (median, std, _) = self.fan_in_stats()?;
        if std == 0.0 {
            return None;
        }
        let v = self.fan_in(path) as f64;
        Some((v - median) / std)
    }

    /// S7.7 — generic project-wide stats for any signal stored on
    /// FileSummary.signals. Returns (median, std, count) or None
    /// when fewer than 2 files have a value for `name`.
    pub fn signal_stats(&self, name: &str) -> Option<(f64, f64, usize)> {
        let values: Vec<f64> = self
            .files
            .values()
            .filter_map(|s| s.signals.get(name).copied())
            .collect();
        descriptive_stats(&values)
    }

    /// Z-score of `path`'s value for `signal_name` against the
    /// workspace baseline. Returns None when stats are unavailable
    /// or std is zero.
    pub fn signal_z_score(&self, path: &Path, signal_name: &str) -> Option<f64> {
        let (median, std, _) = self.signal_stats(signal_name)?;
        if std == 0.0 {
            return None;
        }
        let v = self.files.get(path).and_then(|s| s.signals.get(signal_name))?;
        Some((v - median) / std)
    }

    /// Coarse role classification used by `file_role` payload. Rules
    /// of thumb consistent with the stable-abstractions principle:
    ///   - instability >= 0.8 AND fan_in <= 1  →  "entry"
    ///   - instability <= 0.2 AND fan_in >= 3  →  "core"
    ///   - fan_in >= 3 AND fan_out >= 3        →  "hub"
    ///   - otherwise                           →  "ordinary"
    pub fn role_hint(&self, path: &Path) -> &'static str {
        let fan_in = self.fan_in(path);
        let fan_out = self.fan_out(path);
        let instability = self.instability(path);
        match instability {
            Some(i) if i >= 0.8 && fan_in <= 1 => "entry",
            Some(i) if i <= 0.2 && fan_in >= 3 => "core",
            _ if fan_in >= 3 && fan_out >= 3 => "hub",
            _ => "ordinary",
        }
    }

    /// Does the index contain a module-import cycle?
    pub fn has_cycle(&self) -> bool {
        !self.find_cycle().is_empty()
    }

    /// S3.2: return one cycle as a path of file names (or empty if
    /// none). Used to populate the structured payload of the
    /// `cycle_introduced` reason — the agent can see exactly which
    /// files form the cycle instead of just being told "there's a
    /// cycle somewhere."
    pub fn find_cycle(&self) -> Vec<String> {
        use std::collections::HashMap;
        // Build adjacency by file path.
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for (path, summary) in &self.files {
            let from = path.to_string_lossy().into_owned();
            let mut targets = Vec::new();
            for imp in &summary.imports {
                if let Some(resolved) = resolve_import_to_path(&self.root, path, imp) {
                    targets.push(resolved.to_string_lossy().into_owned());
                }
            }
            adj.insert(from, targets);
        }
        // DFS for back edge.
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color: HashMap<&String, Color> = adj.keys().map(|k| (k, Color::White)).collect();
        let mut stack: Vec<&String> = Vec::new();
        let mut found: Vec<String> = Vec::new();
        let nodes: Vec<&String> = adj.keys().collect();
        for start in nodes {
            if color[start] != Color::White {
                continue;
            }
            // Iterative DFS to keep borrow simple.
            let mut work: Vec<(&String, usize)> = vec![(start, 0)];
            color.insert(start, Color::Gray);
            stack.push(start);
            while let Some(&(node, idx)) = work.last() {
                let neighbors = adj.get(node).map(|v| v.as_slice()).unwrap_or(&[]);
                if idx < neighbors.len() {
                    let next = &neighbors[idx];
                    work.last_mut().unwrap().1 += 1;
                    let Some(next_color) = color.get(next).copied() else {
                        // External target not in our index — skip.
                        continue;
                    };
                    match next_color {
                        Color::White => {
                            // get the &String key from adj that matches `next`.
                            if let Some((key, _)) = adj.get_key_value(next) {
                                color.insert(key, Color::Gray);
                                stack.push(key);
                                work.push((key, 0));
                            }
                        }
                        Color::Gray => {
                            // Back edge → cycle. Reconstruct path from
                            // first occurrence of `next` in stack.
                            if let Some(pos) = stack.iter().position(|s| *s == next) {
                                found.extend(stack[pos..].iter().map(|s| (*s).clone()));
                                if let Some((key, _)) = adj.get_key_value(next) {
                                    found.push(key.clone());
                                }
                            }
                            return found;
                        }
                        Color::Black => {}
                    }
                } else {
                    if let Some(done) = work.pop() {
                        color.insert(done.0, Color::Black);
                        stack.pop();
                    }
                }
            }
        }
        found
    }

    /// Total count of unresolved relative imports across the
    /// whole workspace. Used as a workspace-level cost signal.
    pub fn total_unresolved_imports(&self) -> f64 {
        let mut total: f64 = 0.0;
        for (path, _) in &self.files {
            if let Ok(code) = std::fs::read_to_string(path) {
                total += unresolved_local_import_count(&path.to_string_lossy(), &code);
            }
        }
        total
    }
}

/// Per-file summary: extract imports + public symbol names.
///
/// V2 PR 3: now thin — parses via Layer 1 `parse_file`, then defers
/// to `summarize_file_from_parsed`. Callers that already hold a
/// `ParsedFile` should use `summarize_file_from_parsed` directly to
/// avoid re-parsing.
pub fn summarize_file(path: &Path, code: &str) -> FileSummary {
    let path_str = path.to_string_lossy();
    let Some(parsed) = parse_file(&path_str, code) else {
        return FileSummary::default();
    };
    summarize_file_from_parsed(path, &parsed)
}

/// V2 PR 3: shared infrastructure variant — extract imports + public
/// symbols + per-file numeric signals from a pre-parsed `ParsedFile`.
/// Single tree, single walk per concern. Used by both the
/// disk-driven `summarize_file` path and validate_change's
/// `with_change_from_parsed` path so the workspace cache and the
/// active validation share one parse.
pub fn summarize_file_from_parsed(path: &Path, parsed: &ParsedFile<'_>) -> FileSummary {
    let adapter = parsed.adapter();
    let lang = adapter.tree_sitter_language();
    let src = parsed.source_bytes();
    let root = parsed.root_node();

    // Imports: reuse the language adapter's import_query.
    let mut imports = HashSet::new();
    if let Ok(query) = Query::new(lang, adapter.import_query()) {
        let mut qc = QueryCursor::new();
        for m in qc.matches(&query, root, src) {
            for cap in m.captures {
                if let Ok(text) = cap.node.utf8_text(src) {
                    imports.insert(adapter.normalize_import(text));
                }
            }
        }
    }

    // Public symbols: top-level function / class / const declarations.
    // Heuristic per language; missing the long tail is fine — Ring R2
    // is a delta check, so consistency matters more than completeness.
    let mut public_symbols = HashSet::new();
    extract_public_symbols(root, src, &mut public_symbols);

    let mut imported_symbols = HashSet::new();
    extract_imported_symbols(root, src, &mut imported_symbols);

    // S7.7 — populate per-file numeric signals so project z-scores
    // can be computed without re-parsing each file when stats are
    // queried.
    let mut signals: HashMap<String, f64> = HashMap::new();
    signals.insert("fan_out".into(), imports.len() as f64);
    signals.insert("public_symbols".into(), public_symbols.len() as f64);
    signals.insert("imported_symbols".into(), imported_symbols.len() as f64);
    let s = smell_counts_from_parsed(parsed);
    signals.insert("empty_handler_count".into(), s.empty_handler_count);
    signals.insert("unfinished_marker_count".into(), s.unfinished_marker_count);
    signals.insert("unreachable_stmt_count".into(), s.unreachable_stmt_count);
    signals.insert("cyclomatic_complexity".into(), s.cyclomatic_complexity);
    signals.insert("nesting_depth".into(), s.nesting_depth);
    signals.insert("suspicious_literal_count".into(), s.suspicious_literal_count);
    signals.insert("mutable_default_arg_count".into(), s.mutable_default_arg_count);
    signals.insert("shadowed_local_count".into(), s.shadowed_local_count);
    signals.insert("test_count".into(), s.test_count);
    signals.insert("member_access_count".into(), s.member_access_count);
    signals.insert("type_leakage_count".into(), s.type_leakage_count);
    signals.insert("cross_module_chain_count".into(), s.cross_module_chain_count);
    signals.insert("import_usage_count".into(), s.import_usage_count);
    signals.insert(
        "max_chain_depth".into(),
        adapter.max_chain_depth(root) as f64,
    );
    let _ = path; // path retained in API for future use; not needed by parsed-tree path

    FileSummary {
        imports,
        public_symbols,
        imported_symbols,
        signals,
    }
}

/// Walk the AST and collect names brought in via `from X import Y`
/// or `import { y, z } from 'X'` patterns. Best-effort.
fn extract_imported_symbols(
    node: tree_sitter::Node,
    src: &[u8],
    out: &mut HashSet<String>,
) {
    let kind = node.kind();
    // Python: from X import Y, Z [as W]
    if kind == "import_from_statement" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "dotted_name" => {
                    // Skip the module_name field — only collect the
                    // imported names that follow.
                    if node.child_by_field_name("module_name") == Some(child) {
                        continue;
                    }
                    if let Ok(text) = child.utf8_text(src) {
                        out.insert(text.to_string());
                    }
                }
                "aliased_import" => {
                    if let Some(name_node) = child.named_child(0) {
                        if let Ok(text) = name_node.utf8_text(src) {
                            out.insert(text.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // TS/JS: import { x, y as z } from 'X'
    if kind == "import_specifier" {
        if let Some(name) = node.child_by_field_name("name") {
            if let Ok(text) = name.utf8_text(src) {
                out.insert(text.to_string());
            }
        } else if let Some(first) = node.named_child(0) {
            if let Ok(text) = first.utf8_text(src) {
                out.insert(text.to_string());
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_imported_symbols(child, src, out);
    }
}

fn extract_public_symbols(
    node: tree_sitter::Node,
    src: &[u8],
    out: &mut HashSet<String>,
) {
    let kind = node.kind();
    let is_decl = matches!(
        kind,
        "function_definition" | "function_declaration" | "function_item"
            | "class_definition" | "class_declaration"
            | "method_definition" | "method_declaration"
            | "interface_declaration" | "enum_declaration"
            | "struct_item" | "trait_item" | "type_alias"
            | "lexical_declaration" | "variable_declaration"
            | "export_statement"
    );
    if is_decl {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(src) {
                if is_public_name(name) && is_likely_public(node, src) {
                    out.insert(name.to_string());
                }
            }
        } else if kind == "export_statement" {
            walk_export(node, src, out);
        }
    }
    // Recurse into children, but skip function bodies — nested
    // local helpers are not part of the file's public API.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(kind, "function_definition" | "function_item" | "method_definition")
            && matches!(child.kind(), "block" | "function_body" | "compound_statement")
        {
            continue;
        }
        extract_public_symbols(child, src, out);
    }
}

fn is_public_name(name: &str) -> bool {
    // Python convention: _-prefixed = private.
    !name.starts_with('_')
}

fn is_likely_public(node: tree_sitter::Node, src: &[u8]) -> bool {
    // Rust: must have `pub` modifier.
    if node.kind() == "function_item" || node.kind() == "struct_item" || node.kind() == "trait_item" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "visibility_modifier" {
                if let Ok(text) = child.utf8_text(src) {
                    if text.starts_with("pub") {
                        return true;
                    }
                }
            }
        }
        return false;
    }
    true
}

fn walk_export(node: tree_sitter::Node, src: &[u8], out: &mut HashSet<String>) {
    // S1.8: explicitly mark `export default` exports so callers can
    // detect their loss. The synthetic name "default" is what TS
    // module consumers import via `import Foo from './x'` — the
    // local name on the right is the consumer's choice, but the
    // export slot's identity is "default".
    let raw = node.utf8_text(src).unwrap_or("");
    if raw.trim_start().starts_with("export default") {
        out.insert("default".to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name_node) = child.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(src) {
                if is_public_name(name) {
                    out.insert(name.to_string());
                }
            }
        }
        // For `export { a, b as c } from 'x'` — collect identifiers
        // inside export_clause / export_specifier nodes.
        if matches!(child.kind(), "export_specifier") {
            if let Some(name_node) = child.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(src) {
                    out.insert(name.to_string());
                }
            } else if let Some(first) = child.named_child(0) {
                if let Ok(name) = first.utf8_text(src) {
                    out.insert(name.to_string());
                }
            }
        }
        walk_export(child, src, out);
    }
}

/// Lost public symbols: names that were public in `before` and gone
/// from `after`. Returns the names so the agent can see *what* it
/// broke.
pub fn public_symbols_lost(before: &FileSummary, after: &FileSummary) -> Vec<String> {
    before
        .public_symbols
        .difference(&after.public_symbols)
        .cloned()
        .collect()
}

/// Median + sample standard deviation for a slice of values.
/// Returns None when count < 2. Median used (instead of mean) so a
/// single 5000-line god function file doesn't drag the baseline.
fn descriptive_stats(values: &[f64]) -> Option<(f64, f64, usize)> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len();
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    };
    let mean: f64 = values.iter().sum::<f64>() / n as f64;
    let variance: f64 = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
    let std = variance.sqrt();
    Some((median, std, n))
}

/// Process-global cache of `InMemoryStore<FileSummary>` per workspace
/// root. Populated on first `build_cached` for that root; subsequent
/// calls reuse the same store and only re-parse files whose mtime
/// moved.
fn global_store(root: &Path) -> std::sync::Arc<InMemoryStore<FileSummary>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, std::sync::Arc<InMemoryStore<FileSummary>>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("workspace cache mutex poisoned");
    guard
        .entry(root.to_path_buf())
        .or_insert_with(|| std::sync::Arc::new(InMemoryStore::new()))
        .clone()
}

fn walk_dir_inner(
    dir: &Path,
    files: &mut HashMap<PathBuf, FileSummary>,
    is_root: bool,
) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    if !is_root {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.')
            || matches!(
                name,
                "node_modules" | "target" | "dist" | "build" | "__pycache__" | "venv" | ".venv"
            )
        {
            return Ok(());
        }
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir_inner(&path, files, false)?;
        } else if let Some(_adapter) =
            LanguageRegistry::global().for_path(&path.to_string_lossy())
        {
            if let Ok(code) = std::fs::read_to_string(&path) {
                let summary = summarize_file(&path, &code);
                files.insert(path, summary);
            }
        }
    }
    Ok(())
}

/// Generate the import strings other files would use to refer to
/// `target` (e.g., `./foo`, `.foo`, `crate::foo`). Used by `fan_in`.
///
/// S1.10: previous version used a destructive `trim_end_matches` that
/// stripped the entire filename (e.g., `pkg/sub/foo.py` → `pkg/sub/`),
/// and the Rust path didn't trim non-`.rs` extensions, leaving
/// `crate::pkg/sub/foo.py` as a candidate. Both fixed below.
fn relative_import_targets(root: &Path, target: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem.is_empty() {
        return out;
    }
    // Same-directory shapes (most common).
    out.insert(format!("./{stem}"));
    out.insert(format!(".{stem}"));
    out.insert(stem.to_string());

    if let Ok(rel) = target.strip_prefix(root) {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // Strip the actual extension once — not all alphanumerics.
        let cleaned: &str = match rel_str.rsplit_once('.') {
            Some((head, _ext)) if !head.is_empty() => head,
            _ => &rel_str,
        };
        if !cleaned.is_empty() {
            // Workspace-rooted path forms.
            out.insert(format!("./{cleaned}"));
            out.insert(cleaned.to_string());
            // Python dotted: `pkg.sub.foo`
            let dotted = cleaned.replace('/', ".");
            out.insert(dotted.clone());
            // Python relative parents: `..foo`, `..sub.foo` etc., for
            // callers up to 3 directories away.
            for depth in 1..=3 {
                let dots = ".".repeat(depth);
                if let Some((_pre, tail)) = cleaned.rsplit_once('/') {
                    out.insert(format!("{dots}{}", tail));
                    out.insert(format!("{dots}{}", tail.replace('/', ".")));
                } else {
                    out.insert(format!("{dots}{cleaned}"));
                }
            }
            // Rust: crate::pkg::sub::foo (only if .rs file).
            if rel_str.ends_with(".rs") {
                let rust_path = cleaned
                    .trim_start_matches("src/")
                    .replace('/', "::");
                out.insert(format!("crate::{rust_path}"));
            }
        }
    }
    out
}

fn resolve_import_to_path(root: &Path, from_file: &Path, raw: &str) -> Option<PathBuf> {
    let parent = from_file.parent()?;
    if raw.starts_with("./") || raw.starts_with("../") {
        let candidate = parent.join(raw);
        return locate_with_extensions(&candidate);
    }
    if raw.starts_with('.') {
        // Python relative
        let leading = raw.chars().take_while(|c| *c == '.').count();
        let rest = &raw[leading..];
        let mut path = parent.to_path_buf();
        for _ in 1..leading {
            path = path.parent()?.to_path_buf();
        }
        if rest.is_empty() {
            return Some(path);
        }
        let candidate = path.join(rest.replace('.', "/"));
        return locate_with_extensions(&candidate);
    }
    if raw.starts_with("crate::") {
        let suffix = raw.trim_start_matches("crate::").replace("::", "/");
        let candidate = root.join("src").join(suffix);
        return locate_with_extensions(&candidate);
    }
    None
}

fn locate_with_extensions(base: &Path) -> Option<PathBuf> {
    for ext in ["py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "go", "rs", "java", "cs", "kt", "swift", "dart", "php"] {
        let with_ext = base.with_extension(ext);
        if with_ext.exists() {
            return Some(with_ext);
        }
    }
    if base.is_dir() {
        for index in ["__init__.py", "index.ts", "index.js", "mod.rs"] {
            let p = base.join(index);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn extracts_python_public_symbols() {
        let s = summarize_file(
            Path::new("foo.py"),
            "def public_fn():\n    pass\n\ndef _private():\n    pass\n\nclass Public:\n    pass\n",
        );
        assert!(s.public_symbols.contains("public_fn"));
        assert!(s.public_symbols.contains("Public"));
        assert!(!s.public_symbols.contains("_private"));
    }

    #[test]
    fn detects_lost_public_symbol() {
        let before = summarize_file(
            Path::new("api.py"),
            "def keep():\n    pass\n\ndef will_be_removed():\n    pass\n",
        );
        let after = summarize_file(
            Path::new("api.py"),
            "def keep():\n    pass\n",
        );
        let lost = public_symbols_lost(&before, &after);
        assert!(lost.contains(&"will_be_removed".to_string()));
        assert!(!lost.contains(&"keep".to_string()));
    }

    #[test]
    fn workspace_index_finds_fan_in() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "lib.py", "def helper(): pass\n");
        write(dir.path(), "a.py", "from .lib import helper\n");
        write(dir.path(), "b.py", "from .lib import helper\n");
        let idx = WorkspaceIndex::build(dir.path());
        let fan_in = idx.fan_in(&dir.path().join("lib.py"));
        assert!(fan_in >= 2, "expected fan_in>=2, got {fan_in}");
    }

    #[test]
    fn workspace_index_detects_cycle() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.py", "from .b import x\n");
        write(dir.path(), "b.py", "from .a import y\n");
        let idx = WorkspaceIndex::build(dir.path());
        assert!(idx.has_cycle(), "expected cycle detected");
    }

    #[test]
    fn no_cycle_in_clean_workspace() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.py", "from .b import x\n");
        write(dir.path(), "b.py", "x = 1\n");
        let idx = WorkspaceIndex::build(dir.path());
        assert!(!idx.has_cycle());
    }

    #[test]
    fn signal_z_score_works_for_arbitrary_signal() {
        // S7.7: verify signal_z_score generalizes beyond fan_out.
        // Five files with simple cyclomatic; one with many branches.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..4 {
            write(dir.path(), &format!("simple_{i}.py"), "def f(): return 1\n");
        }
        write(
            dir.path(),
            "complex.py",
            "def f(x):\n    if x: pass\n    elif x==1: pass\n    elif x==2: pass\n    elif x==3: pass\n    elif x==4: pass\n",
        );
        let idx = WorkspaceIndex::build(dir.path());
        let z = idx
            .signal_z_score(&dir.path().join("complex.py"), "cyclomatic_complexity")
            .unwrap();
        assert!(z >= 1.0, "expected z>=1 on complex outlier; got {z}");
    }

    #[test]
    fn fan_out_z_score_marks_outlier() {
        // 4 small files (fan_out 1) + one outlier (fan_out 6) →
        // outlier should have z-score > 1.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..4 {
            write(
                dir.path(),
                &format!("small_{i}.py"),
                "import os\n",
            );
        }
        write(
            dir.path(),
            "outlier.py",
            "import os\nimport sys\nimport json\nimport re\nimport time\nimport math\n",
        );
        let idx = WorkspaceIndex::build(dir.path());
        let z = idx.fan_out_z_score(&dir.path().join("outlier.py")).unwrap();
        assert!(z >= 1.0, "expected z>=1 for outlier, got {z}");
        let z_normal = idx
            .fan_out_z_score(&dir.path().join("small_0.py"))
            .unwrap();
        assert!(z_normal <= 0.0, "expected normal file z<=0, got {z_normal}");
    }

    #[test]
    fn instability_classifies_entry_vs_core() {
        let dir = tempfile::tempdir().unwrap();
        // utils.py is imported by 3 callers; doesn't import anything → core
        write(dir.path(), "utils.py", "def helper(): pass\n");
        write(dir.path(), "a.py", "from .utils import helper\n");
        write(dir.path(), "b.py", "from .utils import helper\n");
        write(dir.path(), "c.py", "from .utils import helper\n");
        // main.py imports many; nothing imports main → entry
        write(
            dir.path(),
            "main.py",
            "from .a import x\nfrom .b import y\nfrom .c import z\nfrom .utils import helper\n",
        );
        let idx = WorkspaceIndex::build(dir.path());

        let utils = dir.path().join("utils.py");
        let utils_i = idx.instability(&utils).unwrap();
        assert!(utils_i <= 0.2, "utils should be stable; got I={utils_i}");
        assert_eq!(idx.role_hint(&utils), "core");

        let main = dir.path().join("main.py");
        let main_i = idx.instability(&main).unwrap();
        assert!(main_i >= 0.8, "main should be unstable; got I={main_i}");
        assert_eq!(idx.role_hint(&main), "entry");
    }

    #[test]
    fn fan_in_works_for_nested_package() {
        // S1.10: previously fan_in undercounted whenever caller and
        // target lived in different package depths.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "pkg/__init__.py", "");
        write(dir.path(), "pkg/sub/__init__.py", "");
        write(dir.path(), "pkg/sub/util.py", "def helper(): pass\n");
        write(dir.path(), "pkg/sibling.py", "from .sub.util import helper\n");
        write(dir.path(), "top_caller.py", "from pkg.sub.util import helper\n");
        let idx = WorkspaceIndex::build(dir.path());
        let target = dir.path().join("pkg/sub/util.py");
        let n = idx.fan_in(&target);
        assert!(n >= 1, "expected fan_in>=1 for nested package, got {n}");
    }

    #[test]
    fn export_default_is_tracked() {
        // S1.8: previously TS `export default class Foo {}` was
        // invisible to public_symbols, so removing it slipped through.
        let s = summarize_file(
            Path::new("foo.ts"),
            "export default class Foo { greet() {} }\n",
        );
        assert!(
            s.public_symbols.contains("default") || s.public_symbols.contains("Foo"),
            "expected `default` or `Foo` in public_symbols; got {:?}",
            s.public_symbols
        );
    }

    #[test]
    fn change_can_introduce_cycle() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.py", "x = 1\n");
        write(dir.path(), "b.py", "from .a import x\n");
        let idx = WorkspaceIndex::build(dir.path());
        assert!(!idx.has_cycle());
        // Hypothetical: a.py starts importing b → cycle.
        let next = idx.with_change(&dir.path().join("a.py"), "from .b import y\n");
        assert!(next.has_cycle(), "expected cycle after change");
    }
}
