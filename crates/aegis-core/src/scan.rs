//! Whole-workspace scan: Ring 0 syntax + Ring 0.5 signals + cross-
//! file import-graph cycle detection. Powers `aegis scan`,
//! `/scan` REPL command, and the `Scan` agent tool.
//!
//! Performance design — both small and large projects fast:
//!   1. **Rayon parallel `par_iter`** — per-file scan work runs
//!      across all CPU cores; rayon's adaptive batching handles
//!      10-file projects (low overhead) and 50k-file projects
//!      (saturates cores) without tuning.
//!   2. **mtime+size cache** at `<workspace>/.aegis-cache/scan.bin`
//!      (bincode-serialised). On re-scan, files unchanged since the
//!      last run reuse the cached `FileScan` — typically 99% of
//!      files in a maintained codebase, giving 100× speed-up.
//!   3. **Defaults skip vendor / build / vcs dirs** so the walk
//!      never descends into `target/` / `node_modules/` etc.
//!
//! Pure observation. Decisions live elsewhere — `aegis scan` prints
//! a report, the chat REPL renders it, `Scan` tool returns text to
//! the LLM. None of these gates anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ast::parser::get_imports_native;
use crate::ast::registry::LanguageRegistry;
use crate::enforcement::check_syntax_native;
use crate::signal_layer_pyapi::extract_signals_native;

/// Bumped whenever `FileScan` semantics or the scanner pipeline
/// changes. Cache entries with a different value are silently
/// invalidated on load.
const SCAN_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub max_files: usize,
    pub skip_dirs: Vec<String>,
    /// Build the import graph + run cycle detection. Default true.
    pub detect_cycles: bool,
    /// Read & write the workspace mtime+size cache. Default true.
    /// Disable for one-off CI scans or to debug stale-cache bugs.
    pub use_cache: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            skip_dirs: vec![
                ".git".into(),
                ".hg".into(),
                ".svn".into(),
                "target".into(),
                "node_modules".into(),
                "dist".into(),
                "build".into(),
                ".venv".into(),
                "venv".into(),
                "__pycache__".into(),
                ".tox".into(),
                ".mypy_cache".into(),
                ".ruff_cache".into(),
                ".pytest_cache".into(),
                ".aegis-cache".into(),
            ],
            detect_cycles: true,
            use_cache: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub root: PathBuf,
    pub files: Vec<FileScan>,
    pub total_cost: f64,
    pub files_with_syntax_errors: usize,
    pub files_scanned: usize,
    pub files_skipped_io_error: usize,
    pub truncated_count: usize,
    /// Each `Vec<PathBuf>` is one strongly-connected component with
    /// ≥2 nodes — i.e. one import cycle. Empty when no cycles.
    pub cyclic_dependencies: Vec<Vec<PathBuf>>,
    pub import_graph: ImportGraphStats,
    /// Stats on cache effectiveness — useful diagnostic for the
    /// "is the cache actually working?" question.
    pub cache_stats: CacheStats,
    /// Wall-clock duration of the scan, in milliseconds.
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportGraphStats {
    pub nodes: usize,
    pub edges: usize,
    pub cycle_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
}

impl ScanReport {
    /// Top-N highest-cost files. Useful for the report summary —
    /// surfaces the worst offenders without dumping every file.
    #[must_use]
    pub fn top_n_by_cost(&self, n: usize) -> Vec<&FileScan> {
        let mut sorted: Vec<&FileScan> = self.files.iter().collect();
        sorted.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));
        sorted.into_iter().take(n).collect()
    }

    #[must_use]
    pub fn syntax_violations(&self) -> Vec<&FileScan> {
        self.files
            .iter()
            .filter(|f| !f.syntax_violations.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileScan {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub cost: f64,
    pub signals: Vec<(String, f64)>,
    pub syntax_violations: Vec<String>,
    pub imports: Vec<String>,
    /// File mtime in nanos-since-epoch + size — used as the cache
    /// key. When both match the on-disk file we trust the cached
    /// `FileScan` (mtime+size missing each other is rare enough in
    /// practice that hashing the content costs more than it saves).
    pub mtime_nanos: i128,
    pub file_size: u64,
}

// ---------- public entry point ----------

/// Walk `root`, scan every supported file (parallel via rayon),
/// detect import cycles, return a complete report.
#[must_use]
pub fn scan_workspace(root: &Path, opts: &ScanOptions) -> ScanReport {
    let start = std::time::Instant::now();
    let supported: Vec<&'static str> = LanguageRegistry::global().extensions();

    // Phase 1 — collect candidate paths.
    let mut paths_to_scan: Vec<PathBuf> = Vec::new();
    walk_collect(root, &supported, &opts.skip_dirs, &mut paths_to_scan);
    let truncated_count = paths_to_scan.len().saturating_sub(opts.max_files);
    paths_to_scan.truncate(opts.max_files);

    // Phase 2 — load cache if enabled.
    let cache = if opts.use_cache {
        load_cache(root)
    } else {
        BTreeMap::new()
    };

    // Phase 3 — parallel per-file scan with cache lookup.
    let cache_ref = &cache;
    let scan_results: Vec<ScanOutcome> = paths_to_scan
        .par_iter()
        .map(|path| scan_one_file(path, root, cache_ref))
        .collect();

    // Phase 4 — accumulate results.
    let mut files: Vec<FileScan> = Vec::with_capacity(scan_results.len());
    let mut files_skipped_io_error = 0usize;
    let mut files_with_syntax_errors = 0usize;
    let mut total_cost = 0.0f64;
    let mut cache_stats = CacheStats::default();

    for outcome in scan_results {
        match outcome {
            ScanOutcome::Cached(file) => {
                cache_stats.hits += 1;
                total_cost += file.cost;
                if !file.syntax_violations.is_empty() {
                    files_with_syntax_errors += 1;
                }
                files.push(file);
            }
            ScanOutcome::Fresh(file) => {
                cache_stats.misses += 1;
                total_cost += file.cost;
                if !file.syntax_violations.is_empty() {
                    files_with_syntax_errors += 1;
                }
                files.push(file);
            }
            ScanOutcome::IoError => {
                files_skipped_io_error += 1;
            }
        }
    }

    let files_scanned = files.len();

    // Phase 5 — write the cache (best-effort; ignore IO errors).
    if opts.use_cache {
        let _ = save_cache(root, &files);
    }

    // Phase 6 — cycle detection (off the hot path; cheap once files
    // are in memory).
    let (cyclic_dependencies, import_graph) = if opts.detect_cycles {
        build_graph_and_find_cycles(&files)
    } else {
        (Vec::new(), ImportGraphStats::default())
    };

    ScanReport {
        root: root.to_path_buf(),
        files,
        total_cost,
        files_with_syntax_errors,
        files_scanned,
        files_skipped_io_error,
        truncated_count,
        cyclic_dependencies,
        import_graph,
        cache_stats,
        duration_ms: start.elapsed().as_millis(),
    }
}

// ---------- per-file scan ----------

enum ScanOutcome {
    Cached(FileScan),
    Fresh(FileScan),
    IoError,
}

/// Cache key: path + mtime + size. Lookup is O(log n) on a BTreeMap
/// keyed by absolute path.
fn scan_one_file(
    path: &Path,
    root: &Path,
    cache: &BTreeMap<PathBuf, FileScan>,
) -> ScanOutcome {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return ScanOutcome::IoError,
    };
    let mtime_nanos = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    let file_size = metadata.len();

    if let Some(cached) = cache.get(path) {
        if cached.mtime_nanos == mtime_nanos && cached.file_size == file_size {
            return ScanOutcome::Cached(cached.clone());
        }
    }

    let path_str = path.to_string_lossy();
    let signals = match extract_signals_native(&path_str) {
        Ok(s) => s,
        Err(_) => return ScanOutcome::IoError,
    };
    let syntax_violations = check_syntax_native(&path_str).unwrap_or_default();
    let imports = get_imports_native(&path_str).unwrap_or_default();
    let cost: f64 = signals.iter().map(|s| s.value).sum();
    let signal_pairs: Vec<(String, f64)> = signals
        .into_iter()
        .map(|s| (s.name, s.value))
        .collect();
    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    ScanOutcome::Fresh(FileScan {
        path: path.to_path_buf(),
        relative_path,
        cost,
        signals: signal_pairs,
        syntax_violations,
        imports,
        mtime_nanos,
        file_size,
    })
}

// ---------- directory walk ----------

fn walk_collect(
    dir: &Path,
    supported: &[&'static str],
    skip_dirs: &[String],
    out: &mut Vec<PathBuf>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            if skip_dirs.iter().any(|s| s == name) {
                continue;
            }
            walk_collect(&path, supported, skip_dirs, out);
        } else if metadata.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            if supported.iter().any(|s| *s == ext) {
                out.push(path);
            }
        }
    }
}

// ---------- import graph + cycle detection ----------

/// String-graph cycle detector. Resolution heuristic: each import
/// string is matched against workspace files by **filename stem**,
/// with **path-context disambiguation** when multiple files share
/// the same stem.
///
/// Catches `a.py imports b, b.py imports a` style cycles, including
/// the realistic case where two unrelated modules both ship a
/// `utils.py` / `helpers.ts` / etc. — the import's prefix segments
/// (`from app.utils import X` → `["app"]`) pick the right candidate
/// instead of the previous "first-wins-on-stem-collision" guess.
///
/// **Bias**: when path context can't disambiguate (bare `import
/// utils` with two `utils.py`s, or no candidate's path matches the
/// import prefix), the edge is **dropped** rather than guessed. A
/// cycle detector that misses some cycles is acceptable; one that
/// fabricates fake cycles in any monorepo with shared module names
/// gets disabled. Cross-language and complex re-export cycles are
/// still out of scope here — flagged as future work.
fn build_graph_and_find_cycles(
    files: &[FileScan],
) -> (Vec<Vec<PathBuf>>, ImportGraphStats) {
    // Multi-value index: stem → all matching file indices. The
    // previous version threw away duplicates with `or_insert`,
    // which silently stitched unrelated `utils.py` files into the
    // same node and produced false cycles.
    let mut stem_to_indices: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in files.iter().enumerate() {
        if let Some(stem) = f.path.file_stem().and_then(|s| s.to_str()) {
            stem_to_indices.entry(stem.to_string()).or_default().push(i);
        }
    }

    let mut graph: DiGraphMap<usize, ()> = DiGraphMap::new();
    let mut edges = 0usize;
    for (i, f) in files.iter().enumerate() {
        graph.add_node(i);
        for imp in &f.imports {
            let target = match resolve_import_to_index(imp, files, &stem_to_indices) {
                Some(t) => t,
                None => continue,
            };
            if target == i {
                continue;
            }
            if !graph.contains_edge(i, target) {
                edges += 1;
                graph.add_edge(i, target, ());
            }
        }
    }

    let nodes = graph.node_count();
    let sccs = tarjan_scc(&graph);
    let mut cycles: Vec<Vec<PathBuf>> = Vec::new();
    for scc in sccs {
        if scc.len() <= 1 {
            continue;
        }
        let mut paths: Vec<PathBuf> = scc
            .into_iter()
            .map(|idx| files[idx].relative_path.clone())
            .collect();
        paths.sort();
        cycles.push(paths);
    }
    let cycle_count = cycles.len();

    (
        cycles,
        ImportGraphStats {
            nodes,
            edges,
            cycle_count,
        },
    )
}

/// Resolve `import_str` to a single file index using path-context
/// disambiguation. Returns `None` when:
///
///   1. No file matches the import's filename stem (out of workspace).
///   2. Multiple files match AND the import string has no path
///      context (bare `import utils` with two `utils.py`s) →
///      ambiguous, skip rather than guess.
///   3. Multiple files match equally well by path-context score →
///      tied, skip.
///   4. Multiple files match but none of them has any ancestor-path
///      segment in common with the import prefix → no useful
///      signal, skip.
///
/// The bias toward false negatives over false positives is
/// deliberate: this is a heuristic feeding a workspace-level report,
/// not a single-file BLOCK gate. A real cycle missed is recoverable
/// by the next deeper analysis tool; a fake cycle reported in a
/// monorepo with shared module names erodes trust in every other
/// finding the report carries.
fn resolve_import_to_index(
    import_str: &str,
    files: &[FileScan],
    stem_to_indices: &BTreeMap<String, Vec<usize>>,
) -> Option<usize> {
    // Strip surrounding quotes (TypeScript / Go / Java string-form
    // imports) and isolate the stem we'll match on.
    let cleaned = import_str.trim_matches(|c: char| c == '"' || c == '\'');
    let last = cleaned
        .rsplit_once(['.', '/'])
        .map(|(_, last)| last)
        .unwrap_or(cleaned);

    let candidates = stem_to_indices.get(last)?;
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    // Path-context disambiguation: split the import's prefix
    // (everything before the matched stem) on `.` / `/` and score
    // each candidate by how many of those segments appear in
    // suffix order at the end of its ancestor directory chain.
    let prefix = cleaned.strip_suffix(last).unwrap_or("");
    let prefix_segments: Vec<&str> = prefix
        .split(['.', '/'])
        .filter(|s| !s.is_empty())
        .collect();

    if prefix_segments.is_empty() {
        // No path context at all + multiple candidates → ambiguous.
        return None;
    }

    let mut best: Option<(usize, usize)> = None; // (winning idx, score)
    let mut tied = false;
    for &idx in candidates {
        let path_segments: Vec<&str> = files[idx]
            .relative_path
            .iter()
            .filter_map(|s| s.to_str())
            .collect();
        // Drop the filename itself; only the ancestor chain
        // counts for path matching.
        let ancestors: &[&str] = if !path_segments.is_empty() {
            &path_segments[..path_segments.len() - 1]
        } else {
            &[]
        };

        let score = ancestors
            .iter()
            .rev()
            .zip(prefix_segments.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();

        match best {
            None => {
                best = Some((idx, score));
                tied = false;
            }
            Some((_, prev)) if score > prev => {
                best = Some((idx, score));
                tied = false;
            }
            Some((_, prev)) if score == prev => {
                tied = true;
            }
            _ => {}
        }
    }

    match best {
        Some((_, 0)) => None, // No candidate had any matching ancestor segments.
        Some(_) if tied => None,
        Some((idx, _)) => Some(idx),
        None => None,
    }
}

// ---------- persistent cache ----------

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: Vec<FileScan>,
}

fn cache_path(root: &Path) -> PathBuf {
    root.join(".aegis-cache").join("scan.bin")
}

fn load_cache(root: &Path) -> BTreeMap<PathBuf, FileScan> {
    let path = cache_path(root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return BTreeMap::new(),
    };
    let cache: CacheFile = match bincode::deserialize(&bytes) {
        Ok(c) => c,
        Err(_) => return BTreeMap::new(),
    };
    if cache.version != SCAN_CACHE_VERSION {
        return BTreeMap::new();
    }
    cache
        .entries
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect()
}

fn save_cache(root: &Path, files: &[FileScan]) -> std::io::Result<()> {
    let path = cache_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let cache = CacheFile {
        version: SCAN_CACHE_VERSION,
        entries: files.to_vec(),
    };
    let bytes = bincode::serialize(&cache).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    std::fs::write(&path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "import os\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "import os\nimport sys\n").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("sub").join("c.py"),
            "import os\nimport sys\nimport json\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join(".venv")).unwrap();
        std::fs::write(dir.path().join(".venv").join("ignored.py"), "x = 1\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi\n").unwrap();
        dir
    }

    #[test]
    fn scan_collects_supported_files_skips_vendor_dirs() {
        let dir = make_workspace();
        let report = scan_workspace(dir.path(), &ScanOptions::default());
        let names: Vec<String> = report
            .files
            .iter()
            .map(|f| f.relative_path.display().to_string())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("a.py")));
        assert!(names.iter().any(|n| n.ends_with("b.py")));
        assert!(names.iter().any(|n| n.ends_with("c.py")));
        assert!(!names.iter().any(|n| n.contains(".venv")));
        assert!(!names.iter().any(|n| n.ends_with("README.md")));
    }

    #[test]
    fn cycle_detected_between_two_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.py"), "import beta\n").unwrap();
        std::fs::write(dir.path().join("beta.py"), "import alpha\n").unwrap();
        let report = scan_workspace(dir.path(), &ScanOptions::default());
        assert_eq!(report.cyclic_dependencies.len(), 1);
        assert_eq!(report.cyclic_dependencies[0].len(), 2);
    }

    #[test]
    fn no_cycle_on_dag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "import b\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "import c\n").unwrap();
        std::fs::write(dir.path().join("c.py"), "x = 1\n").unwrap();
        let report = scan_workspace(dir.path(), &ScanOptions::default());
        assert_eq!(report.cyclic_dependencies.len(), 0);
    }

    #[test]
    fn cycle_disambiguates_by_path_when_multiple_files_share_a_stem() {
        // Realistic monorepo shape: two unrelated `utils.py` modules
        // under different parent dirs. The real cycle is between
        // `app/utils.py` and `app/helpers.py`; the unrelated
        // `db/utils.py` must NOT be pulled into the cycle.
        //
        // Pre-fix behaviour: stem_to_idx kept only the first `utils`
        // entry, so `from app.utils` resolved to whichever entry
        // happened to win the `or_insert` race — non-deterministic
        // and frequently wrong.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("app")).unwrap();
        std::fs::create_dir(dir.path().join("db")).unwrap();
        std::fs::write(
            dir.path().join("app").join("utils.py"),
            "from app.helpers import x\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("app").join("helpers.py"),
            "from app.utils import y\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("db").join("utils.py"),
            "x = 1\n",
        )
        .unwrap();

        let report = scan_workspace(dir.path(), &ScanOptions::default());
        assert_eq!(
            report.cyclic_dependencies.len(),
            1,
            "expected exactly one cycle (app/utils ↔ app/helpers)"
        );
        let cycle_paths: Vec<String> = report.cyclic_dependencies[0]
            .iter()
            .map(|p| p.display().to_string().replace('\\', "/"))
            .collect();
        assert!(
            cycle_paths.iter().any(|p| p.ends_with("app/utils.py")),
            "app/utils.py should be in cycle: {cycle_paths:?}"
        );
        assert!(
            cycle_paths.iter().any(|p| p.ends_with("app/helpers.py")),
            "app/helpers.py should be in cycle: {cycle_paths:?}"
        );
        assert!(
            !cycle_paths.iter().any(|p| p.ends_with("db/utils.py")),
            "db/utils.py must NOT be in cycle: {cycle_paths:?}"
        );
    }

    #[test]
    fn cycle_skips_ambiguous_bare_imports_to_avoid_false_positives() {
        // file.py does a bare `import utils` while two unrelated
        // `utils.py` files exist. Both `app/utils.py` and
        // `db/utils.py` import `file`. Pre-fix, this fabricated a
        // cycle (file ↔ whichever utils won the index race);
        // post-fix the bare import is ambiguous, the edge from
        // file.py is dropped, and no cycle is reported.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("app")).unwrap();
        std::fs::create_dir(dir.path().join("db")).unwrap();
        std::fs::write(dir.path().join("file.py"), "import utils\n").unwrap();
        std::fs::write(
            dir.path().join("app").join("utils.py"),
            "import file\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("db").join("utils.py"),
            "import file\n",
        )
        .unwrap();

        let report = scan_workspace(dir.path(), &ScanOptions::default());
        assert_eq!(
            report.cyclic_dependencies.len(),
            0,
            "ambiguous `import utils` must not fabricate a cycle"
        );
    }

    #[test]
    fn cache_hits_on_second_scan_when_files_unchanged() {
        let dir = make_workspace();
        let opts = ScanOptions::default();

        let first = scan_workspace(dir.path(), &opts);
        assert_eq!(first.cache_stats.hits, 0);
        assert!(first.cache_stats.misses >= 3);

        let second = scan_workspace(dir.path(), &opts);
        // All files unchanged → every entry should be a cache hit.
        assert!(second.cache_stats.hits >= 3);
        assert_eq!(second.cache_stats.misses, 0);
    }

    #[test]
    fn cache_invalidates_when_file_modified() {
        let dir = make_workspace();
        let opts = ScanOptions::default();

        let _ = scan_workspace(dir.path(), &opts);
        // Touch a.py so its mtime advances + content changes size.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(dir.path().join("a.py"), "import os\nimport sys\nimport json\n")
            .unwrap();

        let report = scan_workspace(dir.path(), &opts);
        assert!(report.cache_stats.misses >= 1);
    }

    #[test]
    fn use_cache_false_bypasses_cache_lookup() {
        let dir = make_workspace();
        let _ = scan_workspace(dir.path(), &ScanOptions::default());
        let opts = ScanOptions {
            use_cache: false,
            ..ScanOptions::default()
        };
        let report = scan_workspace(dir.path(), &opts);
        assert_eq!(report.cache_stats.hits, 0);
        assert!(report.cache_stats.misses >= 3);
    }

    #[test]
    fn syntax_error_reported_and_separate_from_io_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok.py"), "x = 1\n").unwrap();
        std::fs::write(dir.path().join("broken.py"), "def f(\n").unwrap();
        let report = scan_workspace(dir.path(), &ScanOptions::default());
        assert!(report.files_with_syntax_errors >= 1);
        assert_eq!(report.files_skipped_io_error, 0);
    }

    #[test]
    fn top_n_orders_by_cost_descending() {
        let dir = make_workspace();
        let report = scan_workspace(dir.path(), &ScanOptions::default());
        let top = report.top_n_by_cost(3);
        for w in top.windows(2) {
            assert!(w[0].cost >= w[1].cost);
        }
    }
}
