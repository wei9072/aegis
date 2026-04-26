//! `Executor` — atomically apply a `PatchPlan`, with backup +
//! rollback on failure.
//!
//! Mirrors `aegis/runtime/executor.py` one-for-one. Assumes the plan
//! has already passed `PlanValidator` (re-verifies at write time
//! since disk may have changed since validation, then rolls back on
//! any issue).
//!
//! Strategy:
//! 1. Build a `Snapshot` over every distinct `patch.path` (in plan
//!    order; deduped). The snapshot records the original on-disk
//!    content (or `None` if the path didn't exist at snapshot time)
//!    and writes a backup directory tree the user can manually
//!    restore from.
//! 2. Apply patches one-by-one, threading the in-memory `current`
//!    map (so MODIFY edits see prior CREATE writes).
//! 3. On any failure (including unexpected exceptions / IO errors),
//!    `Snapshot::restore` puts every touched file back; success
//!    triggers backup-dir GC.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aegis_ir::{apply_edits, is_ok, Patch, PatchKind, PatchPlan, PatchStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::snapshot::{Snapshot, SnapshotError};

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("snapshot error: {0}")]
    Snapshot(#[from] SnapshotError),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchResult {
    pub patch_id: String,
    pub status: PatchStatus,
    #[serde(default)]
    pub matches: usize,
    #[serde(default)]
    pub error: Option<String>,
}

impl PatchResult {
    pub fn ok(patch_id: impl Into<String>, status: PatchStatus, matches: usize) -> Self {
        Self {
            patch_id: patch_id.into(),
            status,
            matches,
            error: None,
        }
    }

    pub fn err(patch_id: impl Into<String>, status: PatchStatus, error: impl Into<String>) -> Self {
        Self {
            patch_id: patch_id.into(),
            status,
            matches: 0,
            error: Some(error.into()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    #[serde(default)]
    pub results: Vec<PatchResult>,
    #[serde(default)]
    pub backup_dir: Option<String>,
    #[serde(default)]
    pub rolled_back: bool,
    /// Reserved for future TOCTOU hash check. Unused today.
    #[serde(default)]
    pub staleness_detected: bool,
    #[serde(default)]
    pub created_paths: Vec<String>,
    #[serde(default)]
    pub touched_paths: Vec<String>,
    /// Final on-disk content per touched path. Populated on success
    /// so ToolCallValidator Tier-2 can compare LLM narration against
    /// what actually got written without re-reading live state
    /// (invariant 6: decision phase consumes only the executor-
    /// provided snapshot).
    #[serde(default)]
    pub path_contents: BTreeMap<String, String>,
}

/// File-system writer + rollback driver.
///
/// The two configurable bits — `backup_subdir` (relative to root,
/// default `.aegis/backups`) and `keep_backups` (default 5) —
/// mirror the V0.x Python `Executor.__init__` signature.
pub struct Executor {
    pub root: PathBuf,
    pub backup_subdir: String,
    pub keep_backups: usize,
}

impl Executor {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            backup_subdir: ".aegis/backups".to_string(),
            keep_backups: 5,
        }
    }

    pub fn with_backup_subdir(mut self, sub: impl Into<String>) -> Self {
        self.backup_subdir = sub.into();
        self
    }

    pub fn with_keep_backups(mut self, n: usize) -> Self {
        self.keep_backups = n;
        self
    }

    /// Apply `plan` atomically. On any failure (including unexpected
    /// IO errors), the snapshot is restored and the returned
    /// `ExecutionResult` carries `success=false, rolled_back=true`.
    pub fn apply(&self, plan: &PatchPlan) -> ExecutionResult {
        let backup_dir = match self.make_backup_dir() {
            Ok(p) => p,
            Err(e) => {
                return ExecutionResult {
                    success: false,
                    results: vec![PatchResult::err(
                        "<pre-apply>",
                        PatchStatus::NotFound,
                        format!("backup dir create failed: {e}"),
                    )],
                    rolled_back: true,
                    ..Default::default()
                };
            }
        };

        // Build the snapshot from distinct patch paths in plan order.
        let mut snapshot = Snapshot::new(self.root.clone());
        for patch in &plan.patches {
            if let Err(e) = snapshot.add(patch.path.clone()) {
                let _ = snapshot.restore();
                return ExecutionResult {
                    success: false,
                    results: vec![PatchResult::err(
                        "<pre-apply>",
                        PatchStatus::NotFound,
                        e.to_string(),
                    )],
                    backup_dir: Some(backup_dir.display().to_string()),
                    rolled_back: true,
                    ..Default::default()
                };
            }
        }
        if let Err(e) = snapshot.write_backup(&backup_dir) {
            let _ = snapshot.restore();
            return ExecutionResult {
                success: false,
                results: vec![PatchResult::err(
                    "<pre-apply>",
                    PatchStatus::NotFound,
                    e.to_string(),
                )],
                backup_dir: Some(backup_dir.display().to_string()),
                rolled_back: true,
                ..Default::default()
            };
        }

        let touched_paths = snapshot.touched_paths();
        let created_paths = snapshot.created_paths();
        // current[path] = post-apply content; None means "deleted".
        let mut current: BTreeMap<String, Option<String>> = BTreeMap::new();
        for path in &touched_paths {
            // Seed current with original content from snapshot via a
            // fresh read — Snapshot doesn't expose entries publicly,
            // so we mirror its observation (file existed at snapshot
            // time = current was Some(content), else None).
            let abs = self.root.join(path);
            current.insert(path.clone(), read_to_string_optional(&abs));
        }

        let (results, failed) = self.apply_patches(plan, &mut current);
        if failed {
            let _ = snapshot.restore();
            return ExecutionResult {
                success: false,
                results,
                backup_dir: Some(backup_dir.display().to_string()),
                rolled_back: true,
                ..Default::default()
            };
        }

        let path_contents: BTreeMap<String, String> = current
            .into_iter()
            .filter_map(|(p, c)| c.map(|content| (p, content)))
            .collect();

        let _ = self.gc_backups();
        ExecutionResult {
            success: true,
            results,
            backup_dir: Some(backup_dir.display().to_string()),
            rolled_back: false,
            staleness_detected: false,
            created_paths,
            touched_paths,
            path_contents,
        }
    }

    /// Restore on-disk state to what it was before `result`.
    /// Restores every file in `result.backup_dir`, deletes every
    /// path in `result.created_paths`. Idempotent.
    pub fn rollback_result(&self, result: &ExecutionResult) -> Result<(), ExecutorError> {
        if let Some(bp) = result.backup_dir.as_deref() {
            let bp = PathBuf::from(bp);
            if bp.is_dir() {
                self.restore_backup_tree(&bp)?;
            }
        }
        for rel in &result.created_paths {
            let abs = self.root.join(rel);
            if abs.exists() {
                fs::remove_file(&abs).map_err(|source| ExecutorError::Io {
                    path: abs,
                    source,
                })?;
            }
        }
        Ok(())
    }

    // ---------- private helpers ----------

    fn apply_patches(
        &self,
        plan: &PatchPlan,
        current: &mut BTreeMap<String, Option<String>>,
    ) -> (Vec<PatchResult>, bool) {
        let mut results = Vec::with_capacity(plan.patches.len());
        for patch in &plan.patches {
            match self.apply_one(patch, current) {
                Ok(result) => {
                    let bad = !is_ok(result.status);
                    results.push(result);
                    if bad {
                        return (results, true);
                    }
                }
                Err(e) => {
                    results.push(PatchResult::err(
                        &patch.id,
                        PatchStatus::NotFound,
                        e.to_string(),
                    ));
                    return (results, true);
                }
            }
        }
        (results, false)
    }

    fn apply_one(
        &self,
        patch: &Patch,
        current: &mut BTreeMap<String, Option<String>>,
    ) -> Result<PatchResult, ExecutorError> {
        let abs = self.root.join(&patch.path);
        let state = current.get(&patch.path).cloned().unwrap_or(None);

        match patch.kind {
            PatchKind::Create => {
                if state.is_some() || abs.exists() {
                    return Ok(PatchResult::err(
                        &patch.id,
                        PatchStatus::NotFound,
                        format!("CREATE target already exists: {}", patch.path),
                    ));
                }
                let content = patch.content.clone().unwrap_or_default();
                if let Some(parent) = abs.parent() {
                    fs::create_dir_all(parent).map_err(|source| ExecutorError::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
                }
                fs::write(&abs, &content).map_err(|source| ExecutorError::Io {
                    path: abs.clone(),
                    source,
                })?;
                current.insert(patch.path.clone(), Some(content));
                Ok(PatchResult::ok(&patch.id, PatchStatus::Applied, 1))
            }

            PatchKind::Modify => {
                let state = match state {
                    Some(s) => s,
                    None => {
                        return Ok(PatchResult::err(
                            &patch.id,
                            PatchStatus::NotFound,
                            format!("MODIFY target missing: {}", patch.path),
                        ));
                    }
                };
                let (new_content, edit_results) = apply_edits(&state, &patch.edits);
                for er in &edit_results {
                    if !is_ok(er.status) {
                        return Ok(PatchResult {
                            patch_id: patch.id.clone(),
                            status: er.status,
                            matches: er.matches,
                            error: None,
                        });
                    }
                }
                let any_applied = edit_results.iter().any(|er| er.status == PatchStatus::Applied);
                let overall = if any_applied {
                    PatchStatus::Applied
                } else {
                    PatchStatus::AlreadyApplied
                };
                if new_content != state {
                    fs::write(&abs, &new_content).map_err(|source| ExecutorError::Io {
                        path: abs.clone(),
                        source,
                    })?;
                }
                current.insert(patch.path.clone(), Some(new_content));
                Ok(PatchResult::ok(&patch.id, overall, 1))
            }

            PatchKind::Delete => {
                if state.is_none() {
                    return Ok(PatchResult::ok(
                        &patch.id,
                        PatchStatus::AlreadyApplied,
                        1,
                    ));
                }
                fs::remove_file(&abs).map_err(|source| ExecutorError::Io {
                    path: abs.clone(),
                    source,
                })?;
                current.insert(patch.path.clone(), None);
                Ok(PatchResult::ok(&patch.id, PatchStatus::Applied, 1))
            }
        }
    }

    fn make_backup_dir(&self) -> Result<PathBuf, ExecutorError> {
        let backup_root = self.root.join(&self.backup_subdir);
        fs::create_dir_all(&backup_root).map_err(|source| ExecutorError::Io {
            path: backup_root.clone(),
            source,
        })?;
        // Python uses `tempfile.mkdtemp(prefix="YYYYMMDD-HHMMSS-")`.
        // Without a tempfile dep we hand-roll a unique name from
        // SystemTime — same shape (`YYYYMMDD-HHMMSS-<nanos>`). The
        // nanos suffix prevents collision when two backups land in
        // the same second.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        let nanos = now.subsec_nanos();
        let tm = format_yyyy_mm_dd_hh_mm_ss(secs);
        for n in 0..1024 {
            let candidate = backup_root.join(format!("{tm}-{nanos:09}-{n:04}"));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(ExecutorError::Io {
                        path: candidate,
                        source,
                    });
                }
            }
        }
        Err(ExecutorError::Io {
            path: backup_root,
            source: io::Error::new(io::ErrorKind::Other, "backup dir name exhausted"),
        })
    }

    fn restore_backup_tree(&self, backup_dir: &Path) -> Result<(), ExecutorError> {
        for entry in walk_files(backup_dir) {
            let entry = entry.map_err(|source| ExecutorError::Io {
                path: backup_dir.to_path_buf(),
                source,
            })?;
            let rel = entry.strip_prefix(backup_dir).unwrap_or(&entry);
            // Skip the deleted-marker artifacts created by
            // Snapshot::write_backup for paths that didn't exist at
            // snapshot time. They aren't real content; restoring them
            // would write a marker file as if it were the original.
            if let Some(ext) = rel.extension() {
                if ext == "deleted_marker" {
                    continue;
                }
            }
            let target = self.root.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| ExecutorError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let bytes = fs::read(&entry).map_err(|source| ExecutorError::Io {
                path: entry.clone(),
                source,
            })?;
            fs::write(&target, bytes).map_err(|source| ExecutorError::Io {
                path: target,
                source,
            })?;
        }
        Ok(())
    }

    fn gc_backups(&self) -> Result<(), ExecutorError> {
        let backup_root = self.root.join(&self.backup_subdir);
        if !backup_root.is_dir() {
            return Ok(());
        }
        let mut dirs: Vec<PathBuf> = fs::read_dir(&backup_root)
            .map_err(|source| ExecutorError::Io {
                path: backup_root.clone(),
                source,
            })?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        if dirs.len() > self.keep_backups {
            for old in &dirs[..dirs.len() - self.keep_backups] {
                let _ = remove_dir_all(old);
            }
        }
        Ok(())
    }
}

fn read_to_string_optional(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok()
}

/// Recursive file-walker (no walkdir dep). Returns absolute paths.
fn walk_files(root: &Path) -> Vec<io::Result<PathBuf>> {
    let mut out = Vec::new();
    fn rec(dir: &Path, out: &mut Vec<io::Result<PathBuf>>) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                out.push(Err(e));
                return;
            }
        };
        for entry in entries {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    if path.is_dir() {
                        rec(&path, out);
                    } else if path.is_file() {
                        out.push(Ok(path));
                    }
                }
                Err(e) => out.push(Err(e)),
            }
        }
    }
    rec(root, &mut out);
    out
}

fn remove_dir_all(p: &Path) -> io::Result<()> {
    fs::remove_dir_all(p)
}

/// Civil-time formatter for the backup-dir prefix. Returns
/// `YYYYMMDD-HHMMSS` in UTC. We're avoiding `chrono`/`time` deps to
/// keep the workspace minimal — the format only needs to be
/// monotonic-ish and human-readable, both of which UTC integer math
/// satisfies.
fn format_yyyy_mm_dd_hh_mm_ss(secs: u64) -> String {
    // Days since 1970-01-01.
    let mut days = (secs / 86_400) as i64;
    let mut sod = secs % 86_400;
    let hour = sod / 3_600;
    sod %= 3_600;
    let minute = sod / 60;
    let second = sod % 60;

    // Convert days → (year, month, day) using the civil-from-days
    // algorithm by Howard Hinnant.
    days += 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}-{hour:02}{minute:02}{second:02}")
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_ir::{Edit, Patch, PatchKind, PatchPlan};
    use tempfile::tempdir;

    fn modify_patch(path: &str, old: &str, new: &str, before: &str, after: &str) -> Patch {
        Patch {
            id: format!("p_{path}"),
            kind: PatchKind::Modify,
            path: path.to_string(),
            rationale: "test".into(),
            content: None,
            edits: vec![Edit::new(old, new).with_context(before, after)],
        }
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    #[test]
    fn modify_patch_applies_and_writes() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![modify_patch("a.py", "original", "renamed", "header\n", "\nfooter")],
            target_files: vec!["a.py".into()],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success, "{result:?}");
        assert_eq!(read(td.path(), "a.py"), "header\nrenamed\nfooter\n");
        assert!(result.backup_dir.is_some());
        assert_eq!(result.touched_paths, vec!["a.py".to_string()]);
        assert!(result.created_paths.is_empty());
    }

    #[test]
    fn create_patch_writes_new_file() {
        let td = tempdir().unwrap();
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Create,
                path: "new/f.py".into(),
                rationale: "".into(),
                content: Some("hello\n".into()),
                edits: vec![],
            }],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success);
        assert_eq!(read(td.path(), "new/f.py"), "hello\n");
        assert_eq!(result.created_paths, vec!["new/f.py".to_string()]);
    }

    #[test]
    fn delete_patch_removes_file() {
        let td = tempdir().unwrap();
        write(td.path(), "doomed.txt", "bye");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Delete,
                path: "doomed.txt".into(),
                rationale: "".into(),
                content: None,
                edits: vec![],
            }],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success);
        assert!(!td.path().join("doomed.txt").exists());
    }

    #[test]
    fn failure_rolls_back_all_files() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        write(td.path(), "b.py", "alpha\nbeta\ngamma\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![
                modify_patch("a.py", "original", "renamed", "header\n", "\nfooter"),
                modify_patch("b.py", "DOES_NOT_EXIST", "x", "nothing\n", "\nnothing"),
            ],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(!result.success);
        assert!(result.rolled_back);
        // both files restored
        assert_eq!(read(td.path(), "a.py"), "header\noriginal\nfooter\n");
        assert_eq!(read(td.path(), "b.py"), "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn rollback_removes_created_files_too() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "hello\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![
                Patch {
                    id: "p1".into(),
                    kind: PatchKind::Create,
                    path: "new.py".into(),
                    rationale: "".into(),
                    content: Some("hello\n".into()),
                    edits: vec![],
                },
                modify_patch("a.py", "NO_SUCH", "x", "nope\n", "\nnope"),
            ],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(!result.success);
        assert!(!td.path().join("new.py").exists());
    }

    #[test]
    fn rerun_returns_already_applied() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![modify_patch("a.py", "original", "renamed", "header\n", "\nfooter")],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let r1 = exe.apply(&plan);
        assert!(r1.success);
        let r2 = exe.apply(&plan);
        assert!(r2.success);
        assert_eq!(r2.results[0].status, PatchStatus::AlreadyApplied);
    }

    #[test]
    fn rollback_result_restores_backup() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![modify_patch("a.py", "original", "renamed", "header\n", "\nfooter")],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success);
        assert_eq!(read(td.path(), "a.py"), "header\nrenamed\nfooter\n");
        exe.rollback_result(&result).unwrap();
        assert_eq!(read(td.path(), "a.py"), "header\noriginal\nfooter\n");
    }

    #[test]
    fn create_then_modify_threads_through_in_memory() {
        let td = tempdir().unwrap();
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![
                Patch {
                    id: "p1".into(),
                    kind: PatchKind::Create,
                    path: "two_step.py".into(),
                    rationale: "".into(),
                    content: Some("v1\nmid\nend\n".into()),
                    edits: vec![],
                },
                modify_patch("two_step.py", "v1", "v2", "", "\nmid"),
            ],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success, "{:?}", result.results);
        assert_eq!(read(td.path(), "two_step.py"), "v2\nmid\nend\n");
    }

    #[test]
    fn path_contents_populated_on_success() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        let exe = Executor::new(td.path());
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![modify_patch("a.py", "original", "renamed", "header\n", "\nfooter")],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        let result = exe.apply(&plan);
        assert!(result.success);
        assert_eq!(
            result.path_contents.get("a.py").map(String::as_str),
            Some("header\nrenamed\nfooter\n"),
        );
    }

    #[test]
    fn gc_drops_old_backups_above_keep_limit() {
        let td = tempdir().unwrap();
        write(td.path(), "a.py", "header\noriginal\nfooter\n");
        let exe = Executor::new(td.path()).with_keep_backups(2);
        let plan = PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches: vec![modify_patch("a.py", "original", "renamed", "header\n", "\nfooter")],
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        };
        // First apply. Subsequent applies become no-op
        // (ALREADY_APPLIED) but still create backup dirs.
        for _ in 0..5 {
            let r = exe.apply(&plan);
            assert!(r.success);
        }
        let backup_root = td.path().join(".aegis").join("backups");
        let dir_count = fs::read_dir(&backup_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(dir_count, 2, "GC should keep only 2 newest backups");
    }
}
