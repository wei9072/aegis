//! Bash tool — `sh -c` subprocess execution with deadline polling
//! plus a V0 redirect parser used by the aegis predictor banner.
//!
//! ## Wire shape
//!
//! ```json
//! { "command": "ls -la" }
//! { "command": "echo x > foo.txt", "timeout_secs": 30 }
//! ```
//!
//! ## Permissions
//!
//! `Bash` lives in `permission::DANGEROUS_TOOLS`, so by default it
//! runs only under `PermissionMode::DangerFullAccess` or via an
//! explicit `Allow` rule. `Plan` mode rejects it (no dry-run
//! semantic for arbitrary shell — what would we even simulate?).
//!
//! ## Aegis-aware redirect parsing (V0)
//!
//! `parse_redirect_targets()` extracts write-destination paths from
//! the most common shell shapes (`>`, `>>`, `tee`, `tee -a`). The
//! predictor uses the result for a stderr banner that names the
//! files this command will touch. V0 does NOT use these targets to
//! BLOCK — that requires content synthesis (knowing what the
//! command would write), which lands incrementally as dogfood
//! reveals real cases worth catching.
//!
//! Documented limitations of the V0 parser (work for V1):
//!   - quoted strings (`echo "a > b"`) — would over-match
//!   - heredocs (`<<EOF`) — not detected
//!   - subshells (`sh -c '...'`) — not recursed
//!   - exec redirects (`2>&1`) — skipped
//!   - no-space forms (`cmd>file`) — not detected
//!
//! Conservative bias: missed targets are silent (no false alarm),
//! over-detection only spends a banner line. PostToolUse cost
//! observer still runs — anything that actually changed structural
//! cost surfaces there even when the parser missed the target.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;
/// Max captured bytes per stream — enough for normal CLI output,
/// caps runaway loops. Truncation marker appended when exceeded.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// Standalone bash executor. Wrap into a `MultiToolExecutor` source
/// alongside `ReadOnlyTools` / `WorkspaceTools` when the agent
/// permits shell.
pub struct BashTool {
    workspace: PathBuf,
}

impl BashTool {
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    /// Single tool definition advertised to the LLM. Stable schema —
    /// the agent prompt references this name.
    #[must_use]
    pub fn definitions() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "Bash".into(),
            description:
                "Run a shell command via /bin/sh -c in the workspace root. \
                 Captures stdout + stderr (truncated to 256 KiB each). \
                 Optional timeout_secs caps execution (default 60, max 600). \
                 Aegis logs any redirect targets (> / >> / tee) before run."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_SECS }
                },
                "required": ["command"]
            }),
        }]
    }

    fn run(&self, input: &str) -> Result<String, ToolError> {
        let parsed: BashInput = serde_json::from_str(input)
            .map_err(|e| ToolError::new(format!("Bash input not valid JSON: {e}")))?;

        let timeout = Duration::from_secs(
            parsed
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .min(MAX_TIMEOUT_SECS),
        );

        let targets = parse_redirect_targets(&parsed.command);
        if !targets.is_empty() {
            // Surface the planned mutation so the user can see it
            // without needing --verbose. Predictor doesn't have
            // content to validate yet (V0); cost observer catches
            // anything that actually changed.
            eprintln!(
                "[aegis] bash will write to: {}",
                targets.join(", ")
            );
        }

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&parsed.command)
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::new(format!("Bash spawn failed: {e}")))?;

        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(ToolError::new(format!(
                            "Bash timed out after {}s: {}",
                            timeout.as_secs(),
                            short(&parsed.command)
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ToolError::new(format!("Bash poll failed: {e}")));
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| ToolError::new(format!("Bash output capture failed: {e}")))?;

        let stdout = capture_with_cap(&output.stdout);
        let stderr = capture_with_cap(&output.stderr);
        let exit = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());

        Ok(format!(
            "exit_code: {exit}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        ))
    }
}

impl ToolExecutor for BashTool {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        match tool_name {
            "Bash" | "bash" => self.run(input),
            other => Err(ToolError::new(format!(
                "BashTool received unknown tool name: {other:?}"
            ))),
        }
    }
}

fn capture_with_cap(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let head = &bytes[..MAX_OUTPUT_BYTES];
    let truncated = bytes.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n... [truncated {truncated} bytes]",
        String::from_utf8_lossy(head)
    )
}

fn short(cmd: &str) -> String {
    const MAX: usize = 80;
    if cmd.len() <= MAX {
        cmd.to_string()
    } else {
        format!("{}…", &cmd[..MAX])
    }
}

/// Extract write-destination paths from a shell command.
///
/// Recognised shapes (whitespace-tokenized):
///
///   - `cmd > path`
///   - `cmd >> path`
///   - `... | tee path`
///   - `... | tee -a path`
///
/// Returns paths in command order, deduplicated. Tokens that look
/// like flags (`-x`, `--long`) are skipped when looking for tee's
/// target.
#[must_use]
pub fn parse_redirect_targets(command: &str) -> Vec<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if t == ">" || t == ">>" {
            if let Some(target) = tokens.get(i + 1) {
                push_unique(&mut found, target);
                i += 2;
                continue;
            }
        } else if t == "tee" {
            // Skip flag tokens to find the path argument.
            let mut j = i + 1;
            while let Some(next) = tokens.get(j) {
                if next.starts_with('-') {
                    j += 1;
                    continue;
                }
                push_unique(&mut found, next);
                break;
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    found
}

fn push_unique(out: &mut Vec<String>, s: &str) {
    let owned = s.to_string();
    if !out.contains(&owned) {
        out.push(owned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn parses_simple_redirect() {
        assert_eq!(
            parse_redirect_targets("echo hello > out.txt"),
            vec!["out.txt".to_string()]
        );
    }

    #[test]
    fn parses_append_redirect() {
        assert_eq!(
            parse_redirect_targets("echo more >> log.txt"),
            vec!["log.txt".to_string()]
        );
    }

    #[test]
    fn parses_tee_target() {
        assert_eq!(
            parse_redirect_targets("ls | tee files.txt"),
            vec!["files.txt".to_string()]
        );
    }

    #[test]
    fn parses_tee_a_skips_flag() {
        assert_eq!(
            parse_redirect_targets("date | tee -a daily.log"),
            vec!["daily.log".to_string()]
        );
    }

    #[test]
    fn parses_multiple_redirects_dedupes() {
        // Same target appearing twice (rare but possible) collapses.
        let r = parse_redirect_targets("echo a > out.txt && echo b >> out.txt");
        assert_eq!(r, vec!["out.txt".to_string()]);
    }

    #[test]
    fn no_redirect_returns_empty() {
        assert!(parse_redirect_targets("ls -la").is_empty());
        assert!(parse_redirect_targets("git status").is_empty());
    }

    #[test]
    fn missing_redirect_target_is_dropped_safely() {
        // Trailing `>` with no following token — nothing to emit.
        assert!(parse_redirect_targets("echo lost >").is_empty());
    }

    #[test]
    fn definitions_advertises_one_tool_named_bash() {
        let defs = BashTool::definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "Bash");
    }

    #[test]
    fn run_simple_command_returns_stdout_and_exit_zero() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({ "command": "echo hello" }).to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("exit_code: 0"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn run_failing_command_captures_nonzero_exit() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({ "command": "exit 7" }).to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("exit_code: 7"));
    }

    #[test]
    fn timeout_kills_long_running_command() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({ "command": "sleep 10", "timeout_secs": 1 }).to_string();
        let err = tool.execute("Bash", &input).unwrap_err();
        assert!(err.message().contains("timed out"));
    }

    #[test]
    fn run_runs_in_workspace_dir() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "x").unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({ "command": "ls" }).to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("marker.txt"));
    }

    #[test]
    fn unknown_tool_name_errors() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let err = tool
            .execute("NotBash", r#"{"command":"echo x"}"#)
            .unwrap_err();
        assert!(err.message().contains("unknown tool name"));
    }

    #[test]
    fn malformed_json_yields_helpful_error() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let err = tool.execute("Bash", "not json").unwrap_err();
        assert!(err.message().contains("not valid JSON"));
    }
}
