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

/// Compose the one-line stderr banner shown to the human when aegis
/// rejects a tool call. Distinct from the structured JSON `reason`
/// returned to the LLM — that goes back through the conversation
/// loop as a tool result; this banner is for the user looking at
/// the terminal so the rejection isn't silent.
///
/// Format examples:
///   `[aegis] BLOCK Edit foo.rs: cost 12 → 15 (+25%); growers: fan_out +2`
///   `[aegis] BLOCK Write bar.py: ring0 invalid_syntax`
///   `[aegis] BLOCK Edit baz.unknown: unsupported_extension`
fn format_block_banner(
    decision: &str,
    reasons: &[Value],
    signals_before: Option<&serde_json::Map<String, Value>>,
    signals_after: Option<&serde_json::Map<String, Value>>,
    regression_detail: Option<&serde_json::Map<String, Value>>,
    tool_name: &str,
    path_display: &str,
) -> String {
    if decision != "BLOCK" {
        return format!("[aegis] {decision} {tool_name} {path_display}");
    }

    // Pick the first reason carrying decision == "block" — that's
    // the headline; remaining ones are noise for the banner.
    let primary = reasons
        .iter()
        .find(|r| r.get("decision").and_then(|d| d.as_str()) == Some("block"));

    let summary = match primary {
        Some(r) => {
            let layer = r.get("layer").and_then(|v| v.as_str()).unwrap_or("?");
            let reason = r.get("reason").and_then(|v| v.as_str()).unwrap_or("?");
            match (layer, reason) {
                ("regression", "cost_increased") => {
                    let before = signals_before
                        .map(sum_map_values)
                        .unwrap_or(0.0);
                    let after = signals_after.map(sum_map_values).unwrap_or(0.0);
                    let pct = if before > 0.0 {
                        ((after - before) / before * 100.0).round() as i64
                    } else {
                        0
                    };
                    let growers = regression_detail
                        .map(|g| {
                            let mut parts: Vec<String> = g
                                .iter()
                                .map(|(k, v)| {
                                    format!("{k} +{}", v.as_f64().unwrap_or(0.0).round() as i64)
                                })
                                .collect();
                            parts.sort();
                            parts.join(", ")
                        })
                        .unwrap_or_default();
                    if growers.is_empty() {
                        format!("cost {before:.0} → {after:.0} (+{pct}%)")
                    } else {
                        format!("cost {before:.0} → {after:.0} (+{pct}%); growers: {growers}")
                    }
                }
                (lyr, rsn) => format!("{lyr} {rsn}"),
            }
        }
        None => "no reason reported".to_string(),
    };

    format!("[aegis] BLOCK {tool_name} {path_display}: {summary}")
}

fn sum_map_values(m: &serde_json::Map<String, Value>) -> f64 {
    m.values().filter_map(|v| v.as_f64()).sum()
}

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
            let reasons_json = parsed
                .get("reasons")
                .map(|r| r.to_string())
                .unwrap_or_else(|| "no reasons reported".into());

            // User-facing banner so the rejection isn't silent on
            // the terminal. The banner is fact-shaped (numbers
            // before / after), not advice — keeps the
            // negative-space framing intact at the UX layer.
            let reasons_array = parsed
                .get("reasons")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let signals_before = parsed
                .get("signals_before")
                .and_then(|v| v.as_object())
                .cloned();
            let signals_after = parsed
                .get("signals_after")
                .and_then(|v| v.as_object())
                .cloned();
            let regression_detail = parsed
                .get("regression_detail")
                .and_then(|v| v.as_object())
                .cloned();
            eprintln!(
                "{}",
                format_block_banner(
                    "BLOCK",
                    &reasons_array,
                    signals_before.as_ref(),
                    signals_after.as_ref(),
                    regression_detail.as_ref(),
                    tool_name,
                    &path,
                )
            );

            return PredictVerdict::Block {
                reason: format!(
                    "aegis predicted BLOCK for {tool_name} on {path}. reasons: {reasons_json}"
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

            eprintln!(
                "{}",
                format_block_banner(
                    &verdict.decision,
                    &verdict.reasons,
                    verdict.signals_before.as_ref(),
                    Some(&verdict.signals_after),
                    verdict.regression_detail.as_ref(),
                    tool_name,
                    &path.display().to_string(),
                )
            );

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn banner_for_ring0_syntax_error() {
        let reasons = vec![json!({
            "layer": "ring0",
            "decision": "block",
            "reason": "invalid_syntax",
        })];
        let line = format_block_banner(
            "BLOCK", &reasons, None, None, None, "Edit", "broken.rs",
        );
        assert_eq!(line, "[aegis] BLOCK Edit broken.rs: ring0 invalid_syntax");
    }

    #[test]
    fn banner_for_unsupported_extension() {
        let reasons = vec![json!({
            "layer": "ring0",
            "decision": "block",
            "reason": "unsupported_extension",
        })];
        let line = format_block_banner(
            "BLOCK", &reasons, None, None, None, "Write", "notes.xyz",
        );
        assert_eq!(line, "[aegis] BLOCK Write notes.xyz: ring0 unsupported_extension");
    }

    #[test]
    fn banner_for_cost_regression_with_growers() {
        let reasons = vec![json!({
            "layer": "regression",
            "decision": "block",
            "reason": "cost_increased",
        })];
        let mut before = Map::new();
        before.insert("fan_out".into(), json!(10.0));
        before.insert("max_chain_depth".into(), json!(2.0));
        let mut after = Map::new();
        after.insert("fan_out".into(), json!(13.0));
        after.insert("max_chain_depth".into(), json!(4.0));
        let mut growers = Map::new();
        growers.insert("fan_out".into(), json!(3.0));
        growers.insert("max_chain_depth".into(), json!(2.0));

        let line = format_block_banner(
            "BLOCK",
            &reasons,
            Some(&before),
            Some(&after),
            Some(&growers),
            "Edit",
            "src/lib.rs",
        );
        // before total = 12, after total = 17, ~+42%
        assert!(
            line.contains("cost 12 → 17 (+42%)") || line.contains("cost 12 → 17 (+41%)"),
            "line: {line}"
        );
        assert!(line.contains("fan_out +3"));
        assert!(line.contains("max_chain_depth +2"));
        assert!(line.starts_with("[aegis] BLOCK Edit src/lib.rs:"));
    }

    #[test]
    fn banner_when_decision_not_block_just_states_decision() {
        let line = format_block_banner(
            "PASS", &[], None, None, None, "Edit", "ok.rs",
        );
        assert_eq!(line, "[aegis] PASS Edit ok.rs");
    }

    #[test]
    fn banner_with_no_reasons_falls_back() {
        let line = format_block_banner(
            "BLOCK", &[], None, None, None, "Edit", "weird.rs",
        );
        assert_eq!(line, "[aegis] BLOCK Edit weird.rs: no reason reported");
    }
}
