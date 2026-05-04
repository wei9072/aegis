//! `aegis-index` — workspace file index with mtime-based caching.
//!
//! S5 architecture work: separates the long-lived "what's in the
//! workspace" state from the short-lived "is this single change
//! safe" judgement (validate.rs). Without this split, every
//! `validate_change_with_workspace` call re-walked the entire tree
//! and re-parsed every file — fine for unit tests, unusable for
//! 10k-file monorepos.
//!
//! Design (intentionally minimal):
//! - `IndexStore<S>` trait: a key/value store from `PathBuf` to a
//!   user-supplied summary type `S`, plus an mtime per entry.
//! - `InMemoryStore<S>`: hash-map implementation. Good enough for
//!   the MCP-daemon and CLI use cases. (Persistent disk-backed
//!   stores can be added behind the same trait without touching
//!   callers.)
//! - `refresh<F>(root, store, parser)`: walks `root`, stat's every
//!   supported file, and only re-invokes `parser(path, content)`
//!   when mtime moved. Calls per `refresh` go from O(N files) to
//!   O(changed files) on warm runs.
//!
//! aegis-core depends on this crate to turn `WorkspaceIndex::build`
//! into a one-line cache hit on warm runs. Other future consumers
//! (background daemons, IDE plugins) re-use the same store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

/// Predicate the index calls to decide whether a directory entry
/// is worth caching. Returning `false` skips it entirely.
pub type FileFilter = dyn Fn(&Path) -> bool + Send + Sync;

/// Per-entry cache record. The summary `S` is whatever your higher
/// layer stores per file (e.g. aegis-core's `FileSummary`).
#[derive(Debug, Clone)]
pub struct Entry<S> {
    pub mtime: SystemTime,
    pub summary: S,
}

/// Trait so callers can swap implementations (in-memory now,
/// sled/sqlite later) without changing the consuming layer.
pub trait IndexStore<S>: Send + Sync {
    fn get(&self, path: &Path) -> Option<Entry<S>>
    where
        S: Clone;
    fn insert(&self, path: PathBuf, entry: Entry<S>);
    fn remove(&self, path: &Path);
    fn paths(&self) -> Vec<PathBuf>;
    fn iter_summaries(&self) -> Vec<(PathBuf, S)>
    where
        S: Clone;
}

/// In-memory implementation. Thread-safe via `Mutex`.
pub struct InMemoryStore<S> {
    inner: Mutex<HashMap<PathBuf, Entry<S>>>,
}

impl<S> Default for InMemoryStore<S> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl<S> InMemoryStore<S> {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry<S>>> {
        self.inner.lock().expect("InMemoryStore mutex poisoned")
    }
}

impl<S: Clone + Send + Sync> IndexStore<S> for InMemoryStore<S> {
    fn get(&self, path: &Path) -> Option<Entry<S>> {
        self.lock().get(path).cloned()
    }

    fn insert(&self, path: PathBuf, entry: Entry<S>) {
        self.lock().insert(path, entry);
    }

    fn remove(&self, path: &Path) {
        self.lock().remove(path);
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.lock().keys().cloned().collect()
    }

    fn iter_summaries(&self) -> Vec<(PathBuf, S)> {
        self.lock()
            .iter()
            .map(|(p, e)| (p.clone(), e.summary.clone()))
            .collect()
    }
}

/// Walk `root`, stat each supported file, and re-invoke
/// `summarize` only on files whose mtime moved (or that aren't
/// in the store yet). Drops cached entries for files that have
/// been deleted from disk.
///
/// Callers pass:
/// - `is_supported(path)`: filter (e.g. "is .py / .ts / .rs").
/// - `summarize(path, content)`: cold-pass parser the first time.
pub fn refresh<S, Filter, Summarize>(
    root: &Path,
    store: &dyn IndexStore<S>,
    is_supported: Filter,
    mut summarize: Summarize,
) -> std::io::Result<()>
where
    S: Clone + Send + Sync,
    Filter: Fn(&Path) -> bool,
    Summarize: FnMut(&Path, &str) -> S,
{
    use std::collections::HashSet;
    let mut seen: HashSet<PathBuf> = HashSet::new();
    walk(root, true, &is_supported, &mut |path| {
        let Ok(meta) = std::fs::metadata(path) else { return };
        let Ok(mtime) = meta.modified() else { return };
        seen.insert(path.to_path_buf());
        if let Some(existing) = store.get(path) {
            if existing.mtime == mtime {
                return; // cache hit
            }
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            let summary = summarize(path, &content);
            store.insert(
                path.to_path_buf(),
                Entry { mtime, summary },
            );
        }
    })?;
    // Drop entries for files that have disappeared.
    for path in store.paths() {
        if !seen.contains(&path) {
            store.remove(&path);
        }
    }
    Ok(())
}

fn walk<F, Visit>(dir: &Path, is_root: bool, filter: &F, visit: &mut Visit) -> std::io::Result<()>
where
    F: Fn(&Path) -> bool,
    Visit: FnMut(&Path),
{
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
            walk(&path, false, filter, visit)?;
        } else if filter(&path) {
            visit(&path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[derive(Clone, Debug, PartialEq)]
    struct LineCount(usize);

    fn make_filter() -> impl Fn(&Path) -> bool {
        |p: &Path| p.extension().and_then(|e| e.to_str()) == Some("txt")
    }

    fn count_lines(_p: &Path, c: &str) -> LineCount {
        LineCount(c.lines().count())
    }

    #[test]
    fn cold_refresh_parses_everything() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "1\n2\n3\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "1\n").unwrap();
        let store: InMemoryStore<LineCount> = InMemoryStore::new();
        refresh(dir.path(), &store, make_filter(), count_lines).unwrap();
        assert_eq!(store.paths().len(), 2);
        let entry = store.get(&dir.path().join("a.txt")).unwrap();
        assert_eq!(entry.summary, LineCount(3));
    }

    #[test]
    fn warm_refresh_skips_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x\n").unwrap();
        let store: InMemoryStore<LineCount> = InMemoryStore::new();
        let mut call_count = 0u32;
        let summarize = |p: &Path, c: &str| {
            call_count += 1;
            count_lines(p, c)
        };
        refresh(dir.path(), &store, make_filter(), summarize).unwrap();
        assert_eq!(call_count, 1);
        // Second refresh — mtime hasn't changed, should skip parse.
        let mut call_count_2 = 0u32;
        let summarize_2 = |p: &Path, c: &str| {
            call_count_2 += 1;
            count_lines(p, c)
        };
        refresh(dir.path(), &store, make_filter(), summarize_2).unwrap();
        assert_eq!(call_count_2, 0, "warm refresh re-parsed unchanged file");
    }

    #[test]
    fn refresh_picks_up_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x\n").unwrap();
        let store: InMemoryStore<LineCount> = InMemoryStore::new();
        refresh(dir.path(), &store, make_filter(), count_lines).unwrap();
        assert_eq!(
            store.get(&path).unwrap().summary,
            LineCount(1),
            "initial"
        );
        // Sleep enough so mtime resolution catches the change on
        // file systems with seconds-only granularity.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, "x\ny\nz\n").unwrap();
        refresh(dir.path(), &store, make_filter(), count_lines).unwrap();
        assert_eq!(
            store.get(&path).unwrap().summary,
            LineCount(3),
            "after edit"
        );
    }

    #[test]
    fn refresh_drops_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x\n").unwrap();
        let store: InMemoryStore<LineCount> = InMemoryStore::new();
        refresh(dir.path(), &store, make_filter(), count_lines).unwrap();
        assert_eq!(store.paths().len(), 1);
        std::fs::remove_file(&path).unwrap();
        refresh(dir.path(), &store, make_filter(), count_lines).unwrap();
        assert_eq!(store.paths().len(), 0, "deleted file should evict");
    }
}
