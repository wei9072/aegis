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
use std::path::Path;

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
