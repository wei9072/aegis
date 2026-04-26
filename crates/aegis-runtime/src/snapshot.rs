//! File-set snapshot + rollback.
//!
//! Mirrors the V0.x Python `Executor` snapshot semantics:
//!
//!   - `Snapshot::take(root, paths)` reads each path's current
//!     content (None if the path doesn't exist) and stores it in
//!     memory. Optionally writes a backup directory tree.
//!   - `Snapshot::restore()` puts every recorded path back to the
//!     state at snapshot time — files that didn't exist get
//!     deleted, files that existed get re-written verbatim.
//!
//! Pure file IO; no patch / plan / language types crosses this
//! boundary. Callers convert their domain-specific patch outputs
//! into a list of paths to snapshot.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("path {path:?} escapes snapshot root {root:?}")]
    PathOutsideRoot { path: PathBuf, root: PathBuf },
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// In-memory record of a set of files at one moment in time.
///
/// Entries: relative path → original content (`None` if the file
/// didn't exist). On `restore` everything goes back: missing-then
/// gets deleted, existing-then gets rewritten.
///
/// `BTreeMap` keeps iteration order deterministic so test snapshots
/// don't hash-randomize.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    root: PathBuf,
    entries: BTreeMap<String, Option<String>>,
}

impl Snapshot {
    /// Empty snapshot anchored at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: BTreeMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn paths(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    /// Number of paths recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Snapshot a set of paths (each relative to `root`). Idempotent
    /// — paths already snapshotted are skipped (mirrors V0.x
    /// `_take_snapshot`'s `if patch.path in snapshot: continue`).
    pub fn capture(
        root: impl Into<PathBuf>,
        rel_paths: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, SnapshotError> {
        let root: PathBuf = root.into();
        let mut snap = Snapshot::new(root.clone());
        for rel in rel_paths {
            snap.add(rel)?;
        }
        Ok(snap)
    }

    pub fn add(&mut self, rel: impl Into<String>) -> Result<(), SnapshotError> {
        let rel: String = rel.into();
        if self.entries.contains_key(&rel) {
            return Ok(());
        }
        let abs = self.root.join(&rel);
        // path-escape guard so callers passing user-controlled
        // strings can't snapshot anything outside root
        let canonical_root = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
        if let Ok(canonical_abs) = abs.canonicalize() {
            if !canonical_abs.starts_with(&canonical_root) {
                return Err(SnapshotError::PathOutsideRoot {
                    path: abs,
                    root: canonical_root,
                });
            }
        }
        let content = match fs::read_to_string(&abs) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(SnapshotError::Io {
                    path: abs,
                    source: e,
                });
            }
        };
        self.entries.insert(rel, content);
        Ok(())
    }

    /// Write each entry's content into `backup_dir`, mirroring the
    /// V0.x Python `_make_backup_dir + per-path file write` loop.
    /// Files that didn't exist at snapshot time get a `<path>.deleted`
    /// marker so a human looking at the backup dir later can tell
    /// what was created versus what was modified.
    pub fn write_backup(&self, backup_dir: &Path) -> Result<(), SnapshotError> {
        fs::create_dir_all(backup_dir).map_err(|e| SnapshotError::Io {
            path: backup_dir.to_path_buf(),
            source: e,
        })?;
        for (rel, content) in &self.entries {
            let dest = backup_dir.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| SnapshotError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
            match content {
                Some(body) => {
                    fs::write(&dest, body).map_err(|e| SnapshotError::Io {
                        path: dest,
                        source: e,
                    })?;
                }
                None => {
                    let marker = dest.with_extension("deleted_marker");
                    fs::write(&marker, b"file did not exist at snapshot time\n").map_err(
                        |e| SnapshotError::Io {
                            path: marker,
                            source: e,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Roll back the on-disk state to what it was at snapshot time.
    /// Idempotent.
    pub fn restore(&self) -> Result<(), SnapshotError> {
        for (rel, original) in &self.entries {
            let abs = self.root.join(rel);
            match original {
                None => {
                    if abs.exists() {
                        fs::remove_file(&abs).map_err(|e| SnapshotError::Io {
                            path: abs,
                            source: e,
                        })?;
                    }
                }
                Some(body) => {
                    if let Some(parent) = abs.parent() {
                        fs::create_dir_all(parent).map_err(|e| SnapshotError::Io {
                            path: parent.to_path_buf(),
                            source: e,
                        })?;
                    }
                    fs::write(&abs, body).map_err(|e| SnapshotError::Io {
                        path: abs,
                        source: e,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Paths that were freshly created since this snapshot — i.e.
    /// the entries whose original content was `None`. Useful for
    /// gating "what did this turn touch".
    pub fn created_paths(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|(p, original)| original.as_ref().map_or(Some(p.clone()), |_| None))
            .collect()
    }

    /// Every path in the snapshot, regardless of whether it existed.
    pub fn touched_paths(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn capture_records_original_content() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.txt"), "hello").unwrap();
        let snap = Snapshot::capture(td.path(), ["a.txt", "missing.txt"]).unwrap();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap.touched_paths().len(), 2);
        assert_eq!(snap.created_paths(), vec!["missing.txt".to_string()]);
    }

    #[test]
    fn restore_undoes_modification() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.txt"), "hello").unwrap();
        let snap = Snapshot::capture(td.path(), ["a.txt"]).unwrap();
        fs::write(td.path().join("a.txt"), "WORLD").unwrap();
        snap.restore().unwrap();
        assert_eq!(fs::read_to_string(td.path().join("a.txt")).unwrap(), "hello");
    }

    #[test]
    fn restore_undoes_creation() {
        let td = tempdir().unwrap();
        let snap = Snapshot::capture(td.path(), ["new.txt"]).unwrap();
        fs::write(td.path().join("new.txt"), "freshly created").unwrap();
        snap.restore().unwrap();
        assert!(!td.path().join("new.txt").exists());
    }

    #[test]
    fn restore_undoes_deletion() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.txt"), "hello").unwrap();
        let snap = Snapshot::capture(td.path(), ["a.txt"]).unwrap();
        fs::remove_file(td.path().join("a.txt")).unwrap();
        snap.restore().unwrap();
        assert_eq!(fs::read_to_string(td.path().join("a.txt")).unwrap(), "hello");
    }

    #[test]
    fn capture_is_idempotent() {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.txt"), "v1").unwrap();
        let mut snap = Snapshot::capture(td.path(), ["a.txt"]).unwrap();
        fs::write(td.path().join("a.txt"), "v2").unwrap();
        // Re-add same path: original v1 must be preserved (mirrors
        // V0.x `if patch.path in snapshot: continue` semantics).
        snap.add("a.txt").unwrap();
        snap.restore().unwrap();
        assert_eq!(fs::read_to_string(td.path().join("a.txt")).unwrap(), "v1");
    }

    #[test]
    fn write_backup_includes_deleted_markers() {
        let td = tempdir().unwrap();
        let backup = td.path().join("backup");
        fs::write(td.path().join("a.txt"), "hello").unwrap();
        let snap = Snapshot::capture(td.path(), ["a.txt", "never_existed.txt"]).unwrap();
        snap.write_backup(&backup).unwrap();
        assert_eq!(fs::read_to_string(backup.join("a.txt")).unwrap(), "hello");
        assert!(backup.join("never_existed.deleted_marker").exists());
    }
}
