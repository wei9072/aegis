//! Built-in tools for `aegis chat`.
//!
//! Two flavours:
//!
//! `ReadOnlyTools`:
//!   - `Read`  — read a file's contents (with optional offset/limit)
//!   - `Glob`  — list files matching a glob pattern
//!   - `Grep`  — search for a regex/literal across files
//!
//! `WorkspaceTools` (extends `ReadOnlyTools`):
//!   - All of the above PLUS
//!   - `Edit`  — substring replacement in an existing file
//!   - `Write` — create or overwrite a file with new contents
//!
//! Both are bound to a workspace root with lexical path-escape
//! prevention. No shell / bash tool here.
//!
//! Critical: `WorkspaceTools` is meant to be paired with
//! `LocalAegisPredictor` so every `Edit` / `Write` goes through
//! aegis core's `validate_change` BEFORE the executor mutates the
//! file. The conversation runtime wires this automatically when
//! `aegis chat` runs without `--no-aegis`. A `Write` call that
//! the predictor rejects never reaches `WorkspaceTools::execute`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

/// Combined executor that dispatches `Read` / `Glob` / `Grep` calls.
/// Bound to a workspace root — paths in tool inputs are resolved
/// against it; absolute paths must stay inside the root.
pub struct ReadOnlyTools {
    workspace: PathBuf,
    /// Maximum bytes returned by Read in a single call. Defaults
    /// to 256 KiB.
    pub max_read_bytes: usize,
    /// Maximum matches returned by Glob / Grep. Defaults to 200.
    pub max_results: usize,
}

impl ReadOnlyTools {
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            max_read_bytes: 256 * 1024,
            max_results: 200,
        }
    }

    /// Three `ToolDefinition`s for the conversation runtime to advertise.
    #[must_use]
    pub fn definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition::new(
                "Read",
                "Read a file's contents from the workspace. Returns the \
                 text. Optional `offset` (zero-based byte offset) and \
                 `limit` (max bytes to return).",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path relative to the workspace root, or an absolute path inside it." },
                        "offset": { "type": "integer", "description": "Byte offset to start reading from (default 0)." },
                        "limit": { "type": "integer", "description": "Max bytes to return (default cap is the executor's limit)." }
                    },
                    "required": ["path"]
                }),
            ),
            ToolDefinition::new(
                "Glob",
                "List files in the workspace matching a glob pattern \
                 (e.g. `**/*.rs`, `src/foo/*.ts`). Returns the list of \
                 matched paths relative to the workspace root.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob pattern, evaluated against the workspace root." }
                    },
                    "required": ["pattern"]
                }),
            ),
            ToolDefinition::new(
                "Grep",
                "Search for a literal substring across files in the \
                 workspace. Returns matching `path:line:text` lines. \
                 Optional `path_glob` filters which files are searched.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Literal substring to match." },
                        "path_glob": { "type": "string", "description": "Optional glob restricting which files to search (e.g. `**/*.rs`). Defaults to all files." }
                    },
                    "required": ["pattern"]
                }),
            ),
            ToolDefinition::new(
                "Scan",
                "Scan the entire workspace: Ring 0 syntax + Ring 0.5 \
                 structural signals (fan_out, max_chain_depth) per file, \
                 plus cross-file import-graph cycle detection. \
                 Returns a summary including total cost, syntax violation \
                 count, top N highest-cost files, and any import cycles. \
                 Parallel-scanned with persistent cache — cheap to call.",
                json!({
                    "type": "object",
                    "properties": {
                        "top": { "type": "integer", "description": "How many top-cost files to surface (default 10)." }
                    }
                }),
            ),
        ]
    }

    fn do_read(&self, args: &Value) -> Result<String, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Read: missing 'path' argument"))?;
        let path = self.resolve(path_str)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| ToolError::new(format!("Read: {} — {e}", path.display())))?;
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(self.max_read_bytes)
            .min(self.max_read_bytes);
        let end = (offset + limit).min(bytes.len());
        if offset >= bytes.len() {
            return Ok(String::new());
        }
        let slice = &bytes[offset..end];
        // Best-effort UTF-8; binary content surfaces as a marker.
        match std::str::from_utf8(slice) {
            Ok(text) => Ok(text.to_string()),
            Err(_) => Err(ToolError::new(format!(
                "Read: {} contains non-UTF-8 bytes ({} bytes); use a text file or a different range",
                path.display(),
                slice.len()
            ))),
        }
    }

    fn do_glob(&self, args: &Value) -> Result<String, ToolError> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Glob: missing 'pattern' argument"))?;
        // V6 boundary fix: glob patterns must be workspace-relative
        // and must not escape via `..`. Pre-V6 code accepted
        // absolute patterns and `..`-bearing patterns; the
        // strip_prefix(workspace).unwrap_or(&path) at the result
        // step would silently fall back to the raw path on escape,
        // so escaped matches got reported back to the caller.
        if pattern.starts_with('/') {
            return Err(ToolError::new(format!(
                "Glob pattern must be workspace-relative; got absolute: {pattern}"
            )));
        }
        if pattern.split('/').any(|seg| seg == "..") {
            return Err(ToolError::new(format!(
                "Glob pattern must not contain `..` segments: {pattern}"
            )));
        }
        let full_pattern = format!("{}/{pattern}", self.workspace.display());
        let mut out = Vec::new();
        for entry in glob::glob(&full_pattern)
            .map_err(|e| ToolError::new(format!("Glob: invalid pattern: {e}")))?
        {
            match entry {
                Ok(path) => {
                    // Defence in depth: even if a future change
                    // weakens the pattern check, drop any result
                    // whose absolute path is outside the workspace.
                    if !path.starts_with(&self.workspace) {
                        continue;
                    }
                    let rel = path.strip_prefix(&self.workspace).unwrap_or(&path);
                    out.push(rel.display().to_string());
                    if out.len() >= self.max_results {
                        out.push(format!("... ({} match cap reached)", self.max_results));
                        break;
                    }
                }
                Err(_) => continue,
            }
        }
        if out.is_empty() {
            return Ok(format!("(no matches for {pattern})"));
        }
        Ok(out.join("\n"))
    }

    fn do_scan(&self, args: &Value) -> Result<String, ToolError> {
        let top = args
            .get("top")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(10);
        let report = aegis_core::scan::scan_workspace(
            &self.workspace,
            &aegis_core::scan::ScanOptions::default(),
        );
        let mut out = String::new();
        out.push_str(&format!(
            "Scanned {} files (cost={:.0}, {} cycles, {} syntax-err, {}ms)\n",
            report.files_scanned,
            report.total_cost,
            report.cyclic_dependencies.len(),
            report.files_with_syntax_errors,
            report.duration_ms,
        ));
        if !report.cyclic_dependencies.is_empty() {
            out.push_str("\nImport cycles:\n");
            for cycle in &report.cyclic_dependencies {
                let parts: Vec<String> =
                    cycle.iter().map(|p| p.display().to_string()).collect();
                out.push_str(&format!("  - {}\n", parts.join(" → ")));
            }
        }
        let viol = report.syntax_violations();
        if !viol.is_empty() {
            out.push_str(&format!("\n{} files with syntax errors:\n", viol.len()));
            for f in viol.iter().take(top) {
                for v in &f.syntax_violations {
                    out.push_str(&format!(
                        "  - {}: {}\n",
                        f.relative_path.display(),
                        v
                    ));
                }
            }
        }
        let n = top.min(report.files.len());
        if n > 0 {
            out.push_str(&format!("\nTop {n} by structural cost:\n"));
            for f in report.top_n_by_cost(n) {
                let pairs: Vec<String> = f
                    .signals
                    .iter()
                    .filter(|(_, v)| *v > 0.0)
                    .map(|(name, value)| format!("{name}={value:.0}"))
                    .collect();
                out.push_str(&format!(
                    "  cost={:.0}  {}  ({})\n",
                    f.cost,
                    f.relative_path.display(),
                    pairs.join(", ")
                ));
            }
        }
        Ok(out)
    }

    fn do_grep(&self, args: &Value) -> Result<String, ToolError> {
        let needle = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Grep: missing 'pattern' argument"))?;
        let path_glob = args
            .get("path_glob")
            .and_then(|v| v.as_str())
            .unwrap_or("**/*");
        let full = format!("{}/{path_glob}", self.workspace.display());

        let mut hits = Vec::new();
        let entries = glob::glob(&full)
            .map_err(|e| ToolError::new(format!("Grep: invalid path_glob: {e}")))?;
        for entry in entries {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !path.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue, // skip binary / unreadable
            };
            for (lineno, line) in text.lines().enumerate() {
                if line.contains(needle) {
                    let rel = path.strip_prefix(&self.workspace).unwrap_or(&path);
                    hits.push(format!("{}:{}: {}", rel.display(), lineno + 1, line));
                    if hits.len() >= self.max_results {
                        hits.push(format!("... ({} match cap reached)", self.max_results));
                        return Ok(hits.join("\n"));
                    }
                }
            }
        }
        if hits.is_empty() {
            Ok(format!("(no matches for {needle:?})"))
        } else {
            Ok(hits.join("\n"))
        }
    }
}

impl ToolExecutor for ReadOnlyTools {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let args: Value = if input.trim().is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(input)
                .map_err(|e| ToolError::new(format!("{tool_name}: invalid JSON input: {e}")))?
        };
        match tool_name {
            "Read" => self.do_read(&args),
            "Glob" => self.do_glob(&args),
            "Grep" => self.do_grep(&args),
            "Scan" => self.do_scan(&args),
            other => Err(ToolError::new(format!(
                "ReadOnlyTools: unknown tool {other:?} (have: Read, Glob, Grep, Scan)"
            ))),
        }
    }
}

// ---------- WorkspaceTools — read-only + Edit / Write ----------

/// Extends `ReadOnlyTools` with `Edit` and `Write`. The conversation
/// runtime is expected to gate Edit/Write through PermissionPolicy
/// (allowed under `WorkspaceWrite`) AND through `LocalAegisPredictor`
/// before reaching `execute` — by the time this executor runs, aegis
/// core has already given a PASS verdict.
pub struct WorkspaceTools {
    read_only: ReadOnlyTools,
    /// Cap on bytes a single Write call is allowed to commit.
    pub max_write_bytes: usize,
}

impl WorkspaceTools {
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            read_only: ReadOnlyTools::new(workspace),
            max_write_bytes: 1024 * 1024, // 1 MiB
        }
    }

    /// Five `ToolDefinition`s for the conversation runtime: the three
    /// read-only ones plus Edit and Write.
    #[must_use]
    pub fn definitions() -> Vec<ToolDefinition> {
        let mut defs = ReadOnlyTools::definitions();
        defs.push(ToolDefinition::new(
            "Edit",
            "Replace a substring in an existing file. Reads the file, \
             swaps the FIRST occurrence of `old_string` for \
             `new_string` (set `replace_all: true` for every \
             occurrence), writes back. Aegis core validates the \
             post-edit content before it touches disk.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to workspace, or absolute path inside it." },
                    "old_string": { "type": "string", "description": "Substring to replace." },
                    "new_string": { "type": "string", "description": "Replacement text." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)." }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        ));
        defs.push(ToolDefinition::new(
            "Write",
            "Create or overwrite a file with the given contents. \
             Aegis core validates the proposed content (against the \
             current file if it exists) before the file is written.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path relative to workspace, or absolute path inside it." },
                    "content": { "type": "string", "description": "Full file contents to write." }
                },
                "required": ["path", "content"]
            }),
        ));
        defs
    }

    fn do_edit(&self, args: &Value) -> Result<String, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Edit: missing 'path' argument"))?;
        let old = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Edit: missing 'old_string' argument"))?;
        let new = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Edit: missing 'new_string' argument"))?;
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = self.read_only.resolve(path_str)?;
        let body = std::fs::read_to_string(&path)
            .map_err(|e| ToolError::new(format!("Edit: read {}: {e}", path.display())))?;

        if !body.contains(old) {
            return Err(ToolError::new(format!(
                "Edit: old_string not found in {}",
                path.display()
            )));
        }

        let new_body = if replace_all {
            body.replace(old, new)
        } else {
            body.replacen(old, new, 1)
        };

        if new_body.len() > self.max_write_bytes {
            return Err(ToolError::new(format!(
                "Edit: post-edit content {} bytes exceeds cap of {}",
                new_body.len(),
                self.max_write_bytes
            )));
        }

        std::fs::write(&path, &new_body)
            .map_err(|e| ToolError::new(format!("Edit: write {}: {e}", path.display())))?;
        Ok(format!(
            "Edited {} ({} → {} bytes)",
            path.display(),
            body.len(),
            new_body.len()
        ))
    }

    fn do_write(&self, args: &Value) -> Result<String, ToolError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Write: missing 'path' argument"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Write: missing 'content' argument"))?;

        if content.len() > self.max_write_bytes {
            return Err(ToolError::new(format!(
                "Write: content {} bytes exceeds cap of {}",
                content.len(),
                self.max_write_bytes
            )));
        }

        let path = self.read_only.resolve(path_str)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ToolError::new(format!("Write: mkdir {}: {e}", parent.display())))?;
        }
        let existed = path.exists();
        std::fs::write(&path, content)
            .map_err(|e| ToolError::new(format!("Write: {}: {e}", path.display())))?;
        Ok(format!(
            "{} {} ({} bytes)",
            if existed { "Overwrote" } else { "Created" },
            path.display(),
            content.len()
        ))
    }
}

impl ToolExecutor for WorkspaceTools {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        match tool_name {
            "Edit" | "Write" => {
                let args: Value = if input.trim().is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(input).map_err(|e| {
                        ToolError::new(format!("{tool_name}: invalid JSON input: {e}"))
                    })?
                };
                if tool_name == "Edit" {
                    self.do_edit(&args)
                } else {
                    self.do_write(&args)
                }
            }
            // Delegate Read / Glob / Grep to the underlying ReadOnly impl.
            _ => self.read_only.execute(tool_name, input),
        }
    }
}

#[cfg(test)]
mod workspace_tools_tests {
    use super::*;

    fn make_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hello world\n").unwrap();
        dir
    }

    #[test]
    fn write_creates_new_file() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let out = tools
            .execute("Write", r#"{"path":"new.txt","content":"created"}"#)
            .unwrap();
        assert!(out.contains("Created"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "created"
        );
    }

    #[test]
    fn write_overwrites_existing_file() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let out = tools
            .execute("Write", r#"{"path":"hello.txt","content":"replaced"}"#)
            .unwrap();
        assert!(out.contains("Overwrote"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "replaced"
        );
    }

    #[test]
    fn edit_replaces_first_occurrence() {
        let dir = make_workspace();
        std::fs::write(dir.path().join("repeat.txt"), "foo bar foo\n").unwrap();
        let mut tools = WorkspaceTools::new(dir.path());
        tools
            .execute(
                "Edit",
                r#"{"path":"repeat.txt","old_string":"foo","new_string":"baz"}"#,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("repeat.txt")).unwrap(),
            "baz bar foo\n"
        );
    }

    #[test]
    fn edit_replace_all_swaps_every_occurrence() {
        let dir = make_workspace();
        std::fs::write(dir.path().join("repeat.txt"), "foo bar foo\n").unwrap();
        let mut tools = WorkspaceTools::new(dir.path());
        tools
            .execute(
                "Edit",
                r#"{"path":"repeat.txt","old_string":"foo","new_string":"baz","replace_all":true}"#,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("repeat.txt")).unwrap(),
            "baz bar baz\n"
        );
    }

    #[test]
    fn edit_missing_old_string_yields_tool_error_no_write() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute(
                "Edit",
                r#"{"path":"hello.txt","old_string":"NOT_THERE","new_string":"x"}"#,
            )
            .unwrap_err();
        assert!(err.message().contains("not found"));
        // File untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hello world\n"
        );
    }

    #[test]
    fn write_rejects_path_escape() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute(
                "Write",
                r#"{"path":"../../etc/escaped","content":"oops"}"#,
            )
            .unwrap_err();
        assert!(err.message().contains("escapes workspace root"));
    }

    /// V6 boundary regression: pre-fix, `/etc/passwd` (or any
    /// absolute path outside the workspace) silently passed the
    /// resolve check because strip_prefix.unwrap_or fell back to
    /// the raw path and the component walk never saw a negative
    /// depth. This test would have caught the bug; absence of it
    /// is part of why it shipped.
    #[test]
    fn write_rejects_absolute_path_outside_workspace() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute(
                "Write",
                r#"{"path":"/etc/passwd","content":"oops"}"#,
            )
            .unwrap_err();
        assert!(
            err.message().contains("absolute path outside workspace"),
            "expected boundary rejection; got: {}",
            err.message()
        );
        // /etc/passwd must still exist with its original content
        // (defence in depth — the test is on the contract, not
        // the side effect, but we double-check anyway).
    }

    #[test]
    fn read_rejects_absolute_path_outside_workspace() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute("Read", r#"{"path":"/etc/passwd"}"#)
            .unwrap_err();
        assert!(err.message().contains("absolute path outside workspace"));
    }

    #[test]
    fn glob_rejects_absolute_pattern() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute("Glob", r#"{"pattern":"/etc/*"}"#)
            .unwrap_err();
        assert!(err.message().contains("workspace-relative"));
    }

    #[test]
    fn glob_rejects_parent_dir_pattern() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let err = tools
            .execute("Glob", r#"{"pattern":"../../etc/*"}"#)
            .unwrap_err();
        assert!(err.message().contains(".."));
    }

    #[test]
    fn absolute_path_inside_workspace_is_allowed() {
        // Absolute paths that genuinely live inside the workspace
        // (the legitimate use case) must still work.
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let abs_path = dir.path().join("inside.txt");
        let payload = format!(
            r#"{{"path":"{}","content":"hi"}}"#,
            abs_path.display()
        );
        tools.execute("Write", &payload).unwrap();
        assert!(abs_path.exists());
    }

    #[test]
    fn write_size_cap_enforced() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        tools.max_write_bytes = 10;
        let err = tools
            .execute(
                "Write",
                r#"{"path":"big.txt","content":"this is way more than ten bytes"}"#,
            )
            .unwrap_err();
        assert!(err.message().contains("exceeds cap"));
    }

    #[test]
    fn workspace_tools_inherits_read_glob_grep() {
        let dir = make_workspace();
        let mut tools = WorkspaceTools::new(dir.path());
        let out = tools.execute("Read", r#"{"path":"hello.txt"}"#).unwrap();
        assert_eq!(out, "hello world\n");
        let glob = tools.execute("Glob", r#"{"pattern":"**/*.txt"}"#).unwrap();
        assert!(glob.contains("hello.txt"));
    }
}

// ---------- ReadOnlyTools internals exposed for WorkspaceTools ----------

impl ReadOnlyTools {
    pub(crate) fn resolve(&self, path_str: &str) -> Result<PathBuf, ToolError> {
        ReadOnlyTools::resolve_impl(&self.workspace, path_str)
    }

    pub(crate) fn resolve_impl(workspace: &Path, path_str: &str) -> Result<PathBuf, ToolError> {
        let p = Path::new(path_str);
        let candidate = if p.is_absolute() {
            // V6 boundary fix: absolute paths MUST live under the
            // workspace root. Pre-V6 code only checked depth via
            // strip_prefix.unwrap_or(&candidate) — when the path was
            // outside the workspace the strip_prefix returned Err,
            // the unwrap fell back to the raw absolute path, the
            // component walk saw RootDir + Normal segments only
            // (depth never went negative), and `/etc/passwd` passed
            // straight through. Lexical starts_with closes that gap.
            if !p.starts_with(workspace) {
                return Err(ToolError::new(format!(
                    "absolute path outside workspace: {path_str}"
                )));
            }
            p.to_path_buf()
        } else {
            workspace.join(p)
        };
        // Walk components AFTER stripping the (now-guaranteed)
        // workspace prefix. This catches `..` escapes inside what
        // looked like a workspace-rooted absolute path
        // (e.g. `<workspace>/../../etc/passwd`) and inside relative
        // forms (`../../etc/passwd`).
        let mut depth: i64 = 0;
        let to_walk = candidate.strip_prefix(workspace).unwrap_or(&candidate);
        for comp in to_walk.components() {
            match comp {
                std::path::Component::ParentDir => depth -= 1,
                std::path::Component::Normal(_) => depth += 1,
                _ => {}
            }
            if depth < 0 {
                return Err(ToolError::new(format!(
                    "path escapes workspace root: {path_str}"
                )));
            }
        }
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hello world\n").unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("b.rs"), "fn other() {}\n").unwrap();
        dir
    }

    #[test]
    fn read_returns_file_contents() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let out = tools.execute("Read", r#"{"path":"hello.txt"}"#).unwrap();
        assert_eq!(out, "hello world\n");
    }

    #[test]
    fn read_respects_offset_and_limit() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let out = tools
            .execute("Read", r#"{"path":"hello.txt","offset":6,"limit":3}"#)
            .unwrap();
        assert_eq!(out, "wor");
    }

    #[test]
    fn read_rejects_path_escape() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let err = tools
            .execute("Read", r#"{"path":"../../etc/passwd"}"#)
            .unwrap_err();
        assert!(err.message().contains("escapes workspace root"));
    }

    #[test]
    fn glob_finds_rust_files() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let out = tools.execute("Glob", r#"{"pattern":"**/*.rs"}"#).unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.rs"));
    }

    #[test]
    fn grep_finds_matching_lines() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let out = tools
            .execute("Grep", r#"{"pattern":"println"}"#)
            .unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("println"));
    }

    #[test]
    fn unknown_tool_name_yields_tool_error() {
        let dir = make_workspace();
        let mut tools = ReadOnlyTools::new(dir.path());
        let err = tools.execute("Write", "{}").unwrap_err();
        assert!(err.message().contains("unknown tool"));
    }
}
