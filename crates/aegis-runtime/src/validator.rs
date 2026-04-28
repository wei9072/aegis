//! `PlanValidator` — gate between Planner and Executor.
//!
//! Catches schema errors, path-safety violations, scope escapes, and
//! cross-patch conflicts (via virtual-filesystem simulation) BEFORE
//! any byte touches disk.
//!
//! Mirrors `aegis/runtime/validator.py` one-for-one. The seven
//! `ErrorKind` discriminants serialize as the exact lowercase strings
//! the Python `Literal` enum used (`"schema"`, `"path"`, `"scope"`,
//! `"dangerous_path"`, `"simulate_not_found"`, `"simulate_ambiguous"`,
//! `"simulate_conflict"`) so any caller pattern-matching on those
//! strings keeps working through the re-export.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use aegis_ir::{apply_edits, is_ok, Patch, PatchKind, PatchPlan, PatchStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Schema,
    Path,
    Scope,
    DangerousPath,
    SimulateNotFound,
    SimulateAmbiguous,
    SimulateConflict,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Schema => "schema",
            ErrorKind::Path => "path",
            ErrorKind::Scope => "scope",
            ErrorKind::DangerousPath => "dangerous_path",
            ErrorKind::SimulateNotFound => "simulate_not_found",
            ErrorKind::SimulateAmbiguous => "simulate_ambiguous",
            ErrorKind::SimulateConflict => "simulate_conflict",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "schema" => ErrorKind::Schema,
            "path" => ErrorKind::Path,
            "scope" => ErrorKind::Scope,
            "dangerous_path" => ErrorKind::DangerousPath,
            "simulate_not_found" => ErrorKind::SimulateNotFound,
            "simulate_ambiguous" => ErrorKind::SimulateAmbiguous,
            "simulate_conflict" => ErrorKind::SimulateConflict,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(default)]
    pub patch_id: Option<String>,
    #[serde(default)]
    pub edit_index: Option<usize>,
    #[serde(default)]
    pub matches: usize,
}

impl ValidationError {
    fn schema(message: impl Into<String>, patch_id: Option<String>) -> Self {
        Self {
            kind: ErrorKind::Schema,
            message: message.into(),
            patch_id,
            edit_index: None,
            matches: 0,
        }
    }

    fn schema_at(message: impl Into<String>, patch_id: String, edit_index: usize) -> Self {
        Self {
            kind: ErrorKind::Schema,
            message: message.into(),
            patch_id: Some(patch_id),
            edit_index: Some(edit_index),
            matches: 0,
        }
    }
}

const FORBIDDEN_PARTS: &[&str] = &[
    ".git",
    ".aegis",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
];

pub struct PlanValidator {
    pub root: PathBuf,
    /// Per-Python: each entry is a path resolved under `root` that
    /// every patch.path must live inside. None means "no scope
    /// constraint".
    pub scope: Option<Vec<PathBuf>>,
}

impl PlanValidator {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let root = root.canonicalize().unwrap_or(root);
        Self { root, scope: None }
    }

    pub fn with_scope(mut self, scope: Vec<String>) -> Result<Self, String> {
        let resolved: Result<Vec<PathBuf>, String> = scope
            .into_iter()
            .map(|s| self.resolve_under_root(&s))
            .collect();
        self.scope = Some(resolved?);
        Ok(self)
    }

    /// Run every check against `plan`; returns the accumulated list
    /// of errors. Empty vec means valid.
    pub fn validate(&self, plan: &PatchPlan) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        errors.extend(self.check_plan_shape(plan));
        for patch in &plan.patches {
            errors.extend(self.check_patch_schema(patch));
            errors.extend(self.check_patch_path(patch));
            errors.extend(self.check_patch_scope(patch));
            errors.extend(self.check_target_files_commitment(patch, plan));
        }
        if !errors.is_empty() {
            return errors;
        }
        errors.extend(self.simulate(plan));
        errors
    }

    fn check_plan_shape(&self, plan: &PatchPlan) -> Vec<ValidationError> {
        let mut errs = Vec::new();
        if plan.patches.is_empty() {
            errs.push(ValidationError::schema("plan has no patches", None));
        }
        let mut seen = HashSet::new();
        let mut dup = false;
        for p in &plan.patches {
            if !seen.insert(p.id.clone()) {
                dup = true;
                break;
            }
        }
        if dup {
            errs.push(ValidationError::schema("patch ids are not unique", None));
        }
        errs
    }

    fn check_patch_schema(&self, patch: &Patch) -> Vec<ValidationError> {
        let mut errs = Vec::new();
        if patch.id.is_empty() {
            errs.push(ValidationError::schema("patch missing id", None));
        }
        if patch.path.is_empty() {
            errs.push(ValidationError::schema(
                "patch missing path",
                Some(patch.id.clone()),
            ));
        }
        match patch.kind {
            PatchKind::Create => {
                if patch.content.is_none() {
                    errs.push(ValidationError::schema(
                        "CREATE patch missing content",
                        Some(patch.id.clone()),
                    ));
                }
                if !patch.edits.is_empty() {
                    errs.push(ValidationError::schema(
                        "CREATE patch must not have edits",
                        Some(patch.id.clone()),
                    ));
                }
            }
            PatchKind::Modify => {
                if patch.edits.is_empty() {
                    errs.push(ValidationError::schema(
                        "MODIFY patch must have at least one edit",
                        Some(patch.id.clone()),
                    ));
                }
                for (i, edit) in patch.edits.iter().enumerate() {
                    if edit.old_string.is_empty() {
                        errs.push(ValidationError::schema_at(
                            "edit has empty old_string",
                            patch.id.clone(),
                            i,
                        ));
                    }
                    if edit.context_before.is_empty() && edit.context_after.is_empty() {
                        errs.push(ValidationError::schema_at(
                            "edit missing context_before and context_after \
                             (at least one required for MODIFY)",
                            patch.id.clone(),
                            i,
                        ));
                    }
                }
            }
            PatchKind::Delete => {
                if patch.content.is_some() || !patch.edits.is_empty() {
                    errs.push(ValidationError::schema(
                        "DELETE patch must not carry content or edits",
                        Some(patch.id.clone()),
                    ));
                }
            }
        }
        errs
    }

    fn check_patch_path(&self, patch: &Patch) -> Vec<ValidationError> {
        if patch.path.is_empty() {
            return Vec::new();
        }
        let resolved = match self.resolve_under_root(&patch.path) {
            Ok(p) => p,
            Err(msg) => {
                return vec![ValidationError {
                    kind: ErrorKind::Path,
                    message: msg,
                    patch_id: Some(patch.id.clone()),
                    edit_index: None,
                    matches: 0,
                }];
            }
        };
        for component in resolved.components() {
            if let Component::Normal(part) = component {
                if let Some(part_str) = part.to_str() {
                    if FORBIDDEN_PARTS.contains(&part_str) {
                        return vec![ValidationError {
                            kind: ErrorKind::DangerousPath,
                            message: format!("path crosses forbidden directory: {part_str}"),
                            patch_id: Some(patch.id.clone()),
                            edit_index: None,
                            matches: 0,
                        }];
                    }
                }
            }
        }
        Vec::new()
    }

    fn check_patch_scope(&self, patch: &Patch) -> Vec<ValidationError> {
        let scope = match &self.scope {
            Some(s) => s,
            None => return Vec::new(),
        };
        if patch.path.is_empty() {
            return Vec::new();
        }
        let resolved = match self.resolve_under_root(&patch.path) {
            Ok(p) => p,
            Err(_) => return Vec::new(), // already reported by check_patch_path
        };
        for allowed in scope {
            if resolved.starts_with(allowed) {
                return Vec::new();
            }
        }
        vec![ValidationError {
            kind: ErrorKind::Scope,
            message: format!(
                "patch path {} outside declared scope",
                patch.path
            ),
            patch_id: Some(patch.id.clone()),
            edit_index: None,
            matches: 0,
        }]
    }

    fn check_target_files_commitment(
        &self,
        patch: &Patch,
        plan: &PatchPlan,
    ) -> Vec<ValidationError> {
        // target_files is a commitment mechanism: the planner declares
        // its intended blast radius, and we reject patches outside
        // that declaration. If empty, no check (planner opted out of
        // the commitment).
        if plan.target_files.is_empty() || patch.path.is_empty() {
            return Vec::new();
        }
        if plan.target_files.iter().any(|t| t == &patch.path) {
            return Vec::new();
        }
        vec![ValidationError {
            kind: ErrorKind::Scope,
            message: format!(
                "patch path {} not in declared target_files {:?}",
                patch.path, plan.target_files
            ),
            patch_id: Some(patch.id.clone()),
            edit_index: None,
            matches: 0,
        }]
    }

    fn simulate(&self, plan: &PatchPlan) -> Vec<ValidationError> {
        let mut errs = Vec::new();
        // virtual[path] = post-application content; None means "deleted".
        // Absent key = "no edits applied; load from disk".
        let mut virtual_fs: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();

        for patch in &plan.patches {
            let current = match virtual_fs.get(&patch.path) {
                Some(v) => v.clone(),
                None => {
                    let abs = self.root.join(&patch.path);
                    if !abs.exists() {
                        None
                    } else {
                        fs::read_to_string(&abs).ok()
                    }
                }
            };

            match patch.kind {
                PatchKind::Create => {
                    if current.is_some() {
                        errs.push(ValidationError {
                            kind: ErrorKind::SimulateConflict,
                            message: format!(
                                "CREATE target already exists: {}",
                                patch.path
                            ),
                            patch_id: Some(patch.id.clone()),
                            edit_index: None,
                            matches: 0,
                        });
                        continue;
                    }
                    virtual_fs.insert(
                        patch.path.clone(),
                        Some(patch.content.clone().unwrap_or_default()),
                    );
                }
                PatchKind::Modify => {
                    let state = match current {
                        Some(s) => s,
                        None => {
                            errs.push(ValidationError {
                                kind: ErrorKind::SimulateConflict,
                                message: format!(
                                    "MODIFY target missing: {}",
                                    patch.path
                                ),
                                patch_id: Some(patch.id.clone()),
                                edit_index: None,
                                matches: 0,
                            });
                            continue;
                        }
                    };
                    let (new_content, results) = apply_edits(&state, &patch.edits);
                    for (i, res) in results.iter().enumerate() {
                        if is_ok(res.status) {
                            continue;
                        }
                        let kind = if res.status == PatchStatus::Ambiguous {
                            ErrorKind::SimulateAmbiguous
                        } else {
                            ErrorKind::SimulateNotFound
                        };
                        errs.push(ValidationError {
                            kind,
                            message: format!(
                                "edit {i} {} (matches={})",
                                res.status.as_str(),
                                res.matches
                            ),
                            patch_id: Some(patch.id.clone()),
                            edit_index: Some(i),
                            matches: res.matches,
                        });
                    }
                    virtual_fs.insert(patch.path.clone(), Some(new_content));
                }
                PatchKind::Delete => {
                    if current.is_none() {
                        errs.push(ValidationError {
                            kind: ErrorKind::SimulateConflict,
                            message: format!(
                                "DELETE target missing: {}",
                                patch.path
                            ),
                            patch_id: Some(patch.id.clone()),
                            edit_index: None,
                            matches: 0,
                        });
                        continue;
                    }
                    virtual_fs.insert(patch.path.clone(), None);
                }
            }
        }
        errs
    }

    /// Lexical resolution under root that does NOT require existence
    /// (CREATE patches reference paths that don't exist yet). Returns
    /// an absolute, normalized path; errors if it would escape root.
    fn resolve_under_root(&self, rel_or_abs: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel_or_abs);
        let candidate = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        let normalized = lexical_normalize(&candidate);
        if !normalized.starts_with(&self.root) {
            return Err(format!("path {rel_or_abs} escapes project root"));
        }
        Ok(normalized)
    }
}

/// Collapse `.` and `..` lexically (no IO; no symlink resolution).
/// Mirrors `os.path.normpath`-ish behaviour. Pops on `..`; ignores
/// `.`. If the input is relative, the output is relative too — but
/// our `resolve_under_root` always passes an absolute path in.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                let popped = out
                    .last()
                    .map(|last| matches!(last, Component::Normal(_)))
                    .unwrap_or(false);
                if popped {
                    out.pop();
                } else {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_ir::{Edit, Patch, PatchKind, PatchPlan};
    use tempfile::tempdir;

    fn workspace() -> tempfile::TempDir {
        let td = tempdir().unwrap();
        fs::write(td.path().join("a.py"), "header\noriginal\nfooter\n").unwrap();
        fs::write(td.path().join("b.py"), "alpha\nbeta\ngamma\n").unwrap();
        td
    }

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

    fn plan_with(patches: Vec<Patch>) -> PatchPlan {
        PatchPlan {
            goal: "g".into(),
            strategy: "s".into(),
            patches,
            target_files: vec![],
            done: false,
            iteration: 0,
            parent_id: None,
        }
    }

    #[test]
    fn accepts_valid_plan() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let mut plan = plan_with(vec![modify_patch(
            "a.py",
            "original",
            "renamed",
            "header\n",
            "\nfooter",
        )]);
        plan.target_files = vec!["a.py".into()];
        assert_eq!(v.validate(&plan), Vec::new());
    }

    #[test]
    fn rejects_path_escape() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![modify_patch(
            "../evil.py",
            "x",
            "y",
            "ctx\n",
            "\nctx",
        )]);
        let errs = v.validate(&plan);
        assert!(errs.iter().any(|e| e.kind == ErrorKind::Path), "{errs:?}");
    }

    #[test]
    fn rejects_dangerous_directory() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![modify_patch(
            ".git/config",
            "a",
            "b",
            "ctx\n",
            "\nctx",
        )]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter().any(|e| e.kind == ErrorKind::DangerousPath),
            "{errs:?}"
        );
    }

    #[test]
    fn enforces_explicit_scope() {
        let td = workspace();
        fs::create_dir(td.path().join("sub")).unwrap();
        fs::write(td.path().join("sub/c.py"), "z = 3\n").unwrap();
        let v = PlanValidator::new(td.path())
            .with_scope(vec!["sub".into()])
            .unwrap();
        let plan = plan_with(vec![modify_patch(
            "a.py",
            "original",
            "renamed",
            "header\n",
            "\nfooter",
        )]);
        let errs = v.validate(&plan);
        assert!(errs.iter().any(|e| e.kind == ErrorKind::Scope), "{errs:?}");
    }

    #[test]
    fn cross_patch_simulation_catches_stale_text() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![
            Patch {
                id: "p1".into(),
                kind: PatchKind::Modify,
                path: "a.py".into(),
                rationale: "r".into(),
                content: None,
                edits: vec![Edit::new("original", "renamed").with_context("header\n", "\nfooter")],
            },
            Patch {
                id: "p2".into(),
                kind: PatchKind::Modify,
                path: "a.py".into(),
                rationale: "r".into(),
                content: None,
                edits: vec![Edit::new("original", "new_name").with_context("header\n", "\nfooter")],
            },
        ]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter().any(|e| e.kind == ErrorKind::SimulateNotFound
                && e.patch_id.as_deref() == Some("p2")),
            "{errs:?}"
        );
    }

    #[test]
    fn enforces_target_files_commitment() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let mut plan = plan_with(vec![modify_patch(
            "b.py",
            "beta",
            "BETA",
            "alpha\n",
            "\ngamma",
        )]);
        plan.target_files = vec!["a.py".into()]; // promised a.py only
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Scope && e.message.contains("target_files")),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_modify_without_context() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![Patch {
            id: "p1".into(),
            kind: PatchKind::Modify,
            path: "a.py".into(),
            rationale: "r".into(),
            content: None,
            edits: vec![Edit::new("original", "renamed")],
        }]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Schema && e.message.contains("context")),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_create_without_content() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![Patch {
            id: "p1".into(),
            kind: PatchKind::Create,
            path: "new.py".into(),
            rationale: "r".into(),
            content: None,
            edits: vec![],
        }]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Schema && e.message.contains("CREATE")),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_delete_with_payload() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![Patch {
            id: "p1".into(),
            kind: PatchKind::Delete,
            path: "a.py".into(),
            rationale: "r".into(),
            content: Some("oops".into()),
            edits: vec![],
        }]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Schema && e.message.contains("DELETE")),
            "{errs:?}"
        );
    }

    #[test]
    fn detects_duplicate_patch_ids() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![
            modify_patch("a.py", "original", "renamed", "header\n", "\nfooter"),
            // Same id as the first patch — even though
            // modify_patch derives id from path, force a collision:
            Patch {
                id: "p_a.py".into(),
                kind: PatchKind::Modify,
                path: "b.py".into(),
                rationale: "".into(),
                content: None,
                edits: vec![Edit::new("beta", "BETA").with_context("alpha\n", "\ngamma")],
            },
        ]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Schema && e.message.contains("unique")),
            "{errs:?}"
        );
    }

    #[test]
    fn empty_plan_fails_schema_check() {
        let td = workspace();
        let v = PlanValidator::new(td.path());
        let plan = plan_with(vec![]);
        let errs = v.validate(&plan);
        assert!(
            errs.iter()
                .any(|e| e.kind == ErrorKind::Schema && e.message.contains("no patches")),
            "{errs:?}"
        );
    }
}
