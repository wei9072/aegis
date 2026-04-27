//! Built-in safe tools for `aegis chat`.
//!
//! Three tools, all read-only by design (no shell, no writes):
//!   - `Read`  — read a file's contents (with optional offset/limit)
//!   - `Glob`  — list files matching a glob pattern
//!   - `Grep`  — search for a regex/literal across files
//!
//! Wired into the `aegis chat` REPL by default. Write/edit/bash
//! tools are deliberately absent here — those land in a later phase
//! when the user opts into `--permission-mode danger-full-access`
//! and we have an explicit need.

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
        ]
    }

    fn resolve(&self, path_str: &str) -> Result<PathBuf, ToolError> {
        let p = Path::new(path_str);
        let candidate = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        };
        // Defensive lexical check (not full canonicalization — the
        // file may not exist yet for a Glob match). Walk components
        // and reject `..` segments that escape the workspace root.
        let mut depth: i64 = 0;
        for comp in candidate.strip_prefix(&self.workspace).unwrap_or(&candidate).components() {
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
        let full_pattern = format!("{}/{pattern}", self.workspace.display());
        let mut out = Vec::new();
        for entry in glob::glob(&full_pattern)
            .map_err(|e| ToolError::new(format!("Glob: invalid pattern: {e}")))?
        {
            match entry {
                Ok(path) => {
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
            other => Err(ToolError::new(format!(
                "ReadOnlyTools: unknown tool {other:?} (have: Read, Glob, Grep)"
            ))),
        }
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
