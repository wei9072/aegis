//! `AegisPredictor` — the concrete predictor that calls aegis-mcp's
//! `validate_change` to predict whether a file-write tool call would
//! BLOCK before letting the runtime execute it.
//!
//! Recognises file-write tool calls by tool name (configurable). Tool
//! calls outside the recognised set pass through unconditionally —
//! the predictor doesn't have an opinion on `read_file`, `bash`, etc.
//!
//! For each recognised call:
//!   1. Parse `path` + `new_content` from the LLM's input.
//!   2. Read current contents from disk if the file exists (becomes
//!      `old_content` for cost regression check).
//!   3. Call `aegis-mcp validate_change(path, new_content, old_content)`.
//!   4. If decision == "BLOCK", return Block with reasons as text.
//!   5. Otherwise Allow.
//!
//! On any internal error (couldn't parse, MCP call failed, etc.) the
//! predictor falls open — Allow with a diagnostic reason. The
//! discipline: **the predictor must never become a single point of
//! agent paralysis**. If aegis-mcp is unreachable, the agent keeps
//! going and the user sees the diagnostic.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::mcp::McpClient;
use crate::predict::{PreToolUsePredictor, PredictVerdict};

/// Default tool names recognised as file-write operations.
/// Extended via `AegisPredictor::watch_tool`.
const DEFAULT_FILE_WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "Edit",
    "Write",
    "MultiEdit",
];

pub struct AegisPredictor {
    mcp: McpClient,
    file_write_tools: BTreeSet<String>,
    /// Last diagnostic from a fall-open path. Useful for tests +
    /// the stdout banner the runtime can choose to emit.
    pub last_diagnostic: Option<String>,
}

impl AegisPredictor {
    #[must_use]
    pub fn new(mcp: McpClient) -> Self {
        Self {
            mcp,
            file_write_tools: DEFAULT_FILE_WRITE_TOOLS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            last_diagnostic: None,
        }
    }

    /// Add another tool name to the watched set.
    pub fn watch_tool(mut self, name: impl Into<String>) -> Self {
        self.file_write_tools.insert(name.into());
        self
    }

    /// Drop the default tool list; useful for tests that want a
    /// known-empty starting set.
    #[must_use]
    pub fn with_only_tools(mcp: McpClient, tools: impl IntoIterator<Item = String>) -> Self {
        Self {
            mcp,
            file_write_tools: tools.into_iter().collect(),
            last_diagnostic: None,
        }
    }
}

impl PreToolUsePredictor for AegisPredictor {
    fn predict(&mut self, tool_name: &str, input: &str) -> PredictVerdict {
        if !self.file_write_tools.contains(tool_name) {
            return PredictVerdict::Allow;
        }

        // Parse the LLM's tool input. If it's malformed, fall open
        // with a diagnostic — the actual tool execution will fail
        // and the LLM will see the real error.
        let args: Value = match serde_json::from_str(input) {
            Ok(v) => v,
            Err(e) => {
                self.last_diagnostic = Some(format!("predict: input not JSON ({e})"));
                return PredictVerdict::Allow;
            }
        };

        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                self.last_diagnostic = Some("predict: input has no 'path' field".into());
                return PredictVerdict::Allow;
            }
        };

        // The LLM may use different field names depending on the
        // tool. Try common ones in priority order.
        let new_content = args
            .get("new_content")
            .or_else(|| args.get("content"))
            .or_else(|| args.get("new_string"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let new_content = match new_content {
            Some(c) => c,
            None => {
                self.last_diagnostic = Some(
                    "predict: no recognised content field (tried new_content / content / new_string)"
                        .into(),
                );
                return PredictVerdict::Allow;
            }
        };

        // Optional: read existing file for cost-regression compare.
        let old_content = std::fs::read_to_string(Path::new(&path)).ok();

        let mut call_args = json!({
            "path": path,
            "new_content": new_content,
        });
        if let Some(old) = &old_content {
            call_args["old_content"] = Value::String(old.clone());
        }

        let result = match self.mcp.call_tool("validate_change", call_args) {
            Ok(r) => r,
            Err(e) => {
                self.last_diagnostic = Some(format!("predict: MCP call_tool failed: {e}"));
                return PredictVerdict::Allow;
            }
        };

        // The text payload is JSON-encoded {decision, reasons, ...}.
        let parsed: Value = match serde_json::from_str(&result.text) {
            Ok(v) => v,
            Err(e) => {
                self.last_diagnostic = Some(format!("predict: verdict not JSON ({e})"));
                return PredictVerdict::Allow;
            }
        };

        let decision = parsed
            .get("decision")
            .and_then(|v| v.as_str())
            .unwrap_or("PASS");
        if decision == "BLOCK" {
            // Surface the reasons array so the LLM sees structured
            // signals — NOT a coaching string, just facts.
            let reasons = parsed
                .get("reasons")
                .map(|r| r.to_string())
                .unwrap_or_else(|| "no reasons reported".into());
            return PredictVerdict::Block {
                reason: format!(
                    "aegis predicted BLOCK for {tool_name} on {path}. reasons: {reasons}"
                ),
            };
        }

        PredictVerdict::Allow
    }
}

// ---------- LocalAegisPredictor — same gates, no subprocess ----------

/// In-process variant of `AegisPredictor`. Calls
/// `aegis_core::validate::validate_change` directly instead of going
/// through an MCP server, so `aegis chat` can ship aegis core
/// gates always-on without requiring users to have the `aegis-mcp`
/// binary installed or wiring a separate `--mcp` flag.
///
/// Behaviour is identical to `AegisPredictor`:
///   - Watches the same default file-write tool names
///     (`Edit` / `Write` / `MultiEdit` / `write_file` / `edit_file`).
///   - Falls open with a `last_diagnostic` on any internal error so
///     the predictor never becomes a single point of agent paralysis.
///   - BLOCK verdict carries the structured `reasons` array so the
///     LLM sees facts, not coaching prose.
pub struct LocalAegisPredictor {
    workspace: PathBuf,
    file_write_tools: BTreeSet<String>,
    pub last_diagnostic: Option<String>,
}

impl LocalAegisPredictor {
    /// `workspace` is used to resolve relative paths in tool inputs.
    /// Absolute paths in tool inputs go through unchanged.
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            file_write_tools: DEFAULT_FILE_WRITE_TOOLS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            last_diagnostic: None,
        }
    }

    pub fn watch_tool(mut self, name: impl Into<String>) -> Self {
        self.file_write_tools.insert(name.into());
        self
    }

    fn resolve(&self, path_str: &str) -> PathBuf {
        let p = Path::new(path_str);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        }
    }

    /// Compose what the file's content WOULD be after the LLM's
    /// proposed tool call, given its input shape.
    ///
    /// Recognises three shapes:
    ///   - `Write` / `write_file`: `{ path, content }` — full file
    ///     replace; `new_content` = `content`.
    ///   - `Edit` (Claude Code style): `{ path, old_string, new_string
    ///     [, replace_all] }` — substring edit; `new_content` =
    ///     existing-disk-content with the substitution applied.
    ///   - `edit_file` / `MultiEdit`: same as Edit, with `edits[]`
    ///     array applied in order.
    ///
    /// Returns `None` (with diagnostic set) when the input doesn't
    /// match any known shape — caller falls open.
    fn synthesize_new_content(
        &mut self,
        tool_name: &str,
        args: &Value,
        path: &Path,
    ) -> Option<String> {
        // Full-file write shape.
        if let Some(content) = args
            .get("content")
            .or_else(|| args.get("new_content"))
            .and_then(|v| v.as_str())
        {
            return Some(content.to_string());
        }

        // Edit shape (single substitution).
        if let (Some(old), Some(new)) = (
            args.get("old_string").and_then(|v| v.as_str()),
            args.get("new_string").and_then(|v| v.as_str()),
        ) {
            let body = std::fs::read_to_string(path).unwrap_or_default();
            let replace_all = args
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let synthesized = if replace_all {
                body.replace(old, new)
            } else {
                body.replacen(old, new, 1)
            };
            return Some(synthesized);
        }

        // MultiEdit shape (sequential substitutions).
        if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
            let mut body = std::fs::read_to_string(path).unwrap_or_default();
            for edit in edits {
                let old = edit.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new = edit.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                let replace_all = edit
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                body = if replace_all {
                    body.replace(old, new)
                } else {
                    body.replacen(old, new, 1)
                };
            }
            return Some(body);
        }

        self.last_diagnostic = Some(format!(
            "predict: tool {tool_name:?} input has no recognised content shape"
        ));
        None
    }
}

impl PreToolUsePredictor for LocalAegisPredictor {
    fn predict(&mut self, tool_name: &str, input: &str) -> PredictVerdict {
        if !self.file_write_tools.contains(tool_name) {
            return PredictVerdict::Allow;
        }

        let args: Value = match serde_json::from_str(input) {
            Ok(v) => v,
            Err(e) => {
                self.last_diagnostic = Some(format!("predict: input not JSON ({e})"));
                return PredictVerdict::Allow;
            }
        };

        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => {
                self.last_diagnostic = Some("predict: input has no 'path' field".into());
                return PredictVerdict::Allow;
            }
        };

        let path = self.resolve(&path_str);
        let new_content = match self.synthesize_new_content(tool_name, &args, &path) {
            Some(c) => c,
            None => return PredictVerdict::Allow,
        };
        let old_content = std::fs::read_to_string(&path).ok();

        // Drive aegis core's validate_change — same gates as the
        // MCP-based AegisPredictor, just no JSON-RPC round trip.
        let verdict = aegis_core::validate::validate_change(
            path.to_string_lossy().as_ref(),
            &new_content,
            old_content.as_deref(),
        );

        if verdict.blocked() {
            let reasons_summary = serde_json::to_string(&verdict.reasons)
                .unwrap_or_else(|_| "no reasons reported".into());
            return PredictVerdict::Block {
                reason: format!(
                    "aegis predicted BLOCK for {tool_name} on {}. reasons: {reasons_summary}",
                    path.display()
                ),
            };
        }

        PredictVerdict::Allow
    }
}
