//! Permission system — V3.6 parity with Claude Code's three modes.
//!
//! Mode summary:
//!   - `ReadOnly`         — only safe inspection tools allowed
//!   - `WorkspaceWrite`   — read + edit/write inside the workspace
//!   - `DangerFullAccess` — anything goes (used for `bash`, network,
//!                          arbitrary system access)
//!
//! Wired into ConversationRuntime as another PreToolUse gate. Runs
//! BEFORE the aegis-predict gate (no point asking aegis-mcp about
//! something the user has banned). Tool denials surface to the LLM
//! as `is_error=true` tool_results — the LLM sees the denial and
//! decides what to try next; the runtime never coaches.

use std::collections::BTreeSet;

/// Permission modes covering inspection-only through full-access.
/// `Plan` is aegis-specific: write tools route through the predictor
/// for cost-delta scoring but are NOT executed — see `Plan` doc and
/// `PermissionDecision::AllowDryRun`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionMode {
    /// Only allow read-only inspection tools (read_file, glob,
    /// grep, list, etc.). Everything else denied.
    ReadOnly,
    /// Allow reads + write/edit inside the workspace. Reject bash,
    /// network, anything that could leak outside.
    WorkspaceWrite,
    /// Read tools execute normally; write tools dry-run — predictor
    /// scores the structural cost delta and returns a synthesized
    /// "plan: would write to X (+Y cost)" tool result to the LLM
    /// instead of touching the disk. The framing-correct alternative
    /// to claw-code's prose-only plan summary: aegis's plan summary
    /// is structural numbers, not LLM narrative.
    Plan,
    /// All tools allowed. Used when the user explicitly accepts
    /// risk (e.g. running an unattended ticket where the agent
    /// needs to run tests via bash).
    DangerFullAccess,
}

/// Policy decision for one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool is permitted; runtime executes it normally.
    Allow,
    /// Tool is permitted but must NOT execute against disk / shell —
    /// runtime calls the predictor for structural-cost scoring and
    /// returns a synthesized result. Used by `Plan` mode.
    AllowDryRun,
    /// Tool blocked. `reason` goes back to the LLM as the tool
    /// result — facts only, no coaching.
    Deny { reason: String },
}

/// Default tool-name classification used by the built-in
/// `PermissionPolicy::standard`. Override via the builder methods.
const READ_TOOLS: &[&str] = &[
    "read_file", "Read",
    "glob", "Glob", "glob_search",
    "grep", "Grep", "grep_search",
    "list_dir", "ls",
];
const WRITE_TOOLS: &[&str] = &[
    "write_file", "Write",
    "edit_file", "Edit",
    "MultiEdit",
];
const DANGEROUS_TOOLS: &[&str] = &["bash", "Bash", "shell", "execute_command"];

#[derive(Clone, Debug)]
pub struct PermissionPolicy {
    mode: PermissionMode,
    read_tools: BTreeSet<String>,
    write_tools: BTreeSet<String>,
    dangerous_tools: BTreeSet<String>,
}

impl PermissionPolicy {
    /// Standard policy — uses the default tool-name lists above.
    #[must_use]
    pub fn standard(mode: PermissionMode) -> Self {
        Self {
            mode,
            read_tools: READ_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            write_tools: WRITE_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            dangerous_tools: DANGEROUS_TOOLS.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn allow_read(mut self, tool: impl Into<String>) -> Self {
        self.read_tools.insert(tool.into());
        self
    }

    pub fn allow_write(mut self, tool: impl Into<String>) -> Self {
        self.write_tools.insert(tool.into());
        self
    }

    pub fn allow_dangerous(mut self, tool: impl Into<String>) -> Self {
        self.dangerous_tools.insert(tool.into());
        self
    }

    /// Return whether the given tool is permitted under the current
    /// mode. Tools NOT in any list are treated like dangerous tools
    /// (deny unless DangerFullAccess) — the safe default.
    pub fn authorize(&self, tool_name: &str) -> PermissionDecision {
        let is_read = self.read_tools.contains(tool_name);
        let is_write = self.write_tools.contains(tool_name);
        let is_dangerous = self.dangerous_tools.contains(tool_name);
        let unknown = !is_read && !is_write && !is_dangerous;

        match self.mode {
            PermissionMode::ReadOnly => {
                if is_read {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: format!(
                            "permission denied: tool {tool_name:?} not allowed in ReadOnly mode"
                        ),
                    }
                }
            }
            PermissionMode::WorkspaceWrite => {
                if is_read || is_write {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: format!(
                            "permission denied: tool {tool_name:?} not allowed in WorkspaceWrite mode (use DangerFullAccess for shell / unknown tools)"
                        ),
                    }
                }
            }
            PermissionMode::Plan => {
                if is_read {
                    // Reads are safe; let them through normally.
                    PermissionDecision::Allow
                } else if is_write {
                    // The hallmark of plan mode: write tools score
                    // through the predictor but never touch disk.
                    PermissionDecision::AllowDryRun
                } else {
                    PermissionDecision::Deny {
                        reason: format!(
                            "permission denied: tool {tool_name:?} not allowed in Plan mode (only read tools execute; write tools dry-run; everything else blocked)"
                        ),
                    }
                }
            }
            PermissionMode::DangerFullAccess => {
                let _ = unknown; // unknown tools allowed in this mode
                PermissionDecision::Allow
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_mode_allows_read_denies_write_and_bash() {
        let p = PermissionPolicy::standard(PermissionMode::ReadOnly);
        assert_eq!(p.authorize("read_file"), PermissionDecision::Allow);
        assert_eq!(p.authorize("glob"), PermissionDecision::Allow);
        assert!(matches!(
            p.authorize("write_file"),
            PermissionDecision::Deny { .. }
        ));
        assert!(matches!(p.authorize("bash"), PermissionDecision::Deny { .. }));
    }

    #[test]
    fn workspace_write_allows_read_and_write_denies_bash() {
        let p = PermissionPolicy::standard(PermissionMode::WorkspaceWrite);
        assert_eq!(p.authorize("Edit"), PermissionDecision::Allow);
        assert_eq!(p.authorize("read_file"), PermissionDecision::Allow);
        assert!(matches!(p.authorize("bash"), PermissionDecision::Deny { .. }));
    }

    #[test]
    fn danger_full_access_allows_everything_including_unknown() {
        let p = PermissionPolicy::standard(PermissionMode::DangerFullAccess);
        assert_eq!(p.authorize("bash"), PermissionDecision::Allow);
        assert_eq!(p.authorize("unknown_tool"), PermissionDecision::Allow);
    }

    #[test]
    fn workspace_write_denies_unknown_tool_safe_default() {
        // Unknown tools are not categorised — safe default is deny
        // unless DangerFullAccess.
        let p = PermissionPolicy::standard(PermissionMode::WorkspaceWrite);
        assert!(matches!(
            p.authorize("totally_new_tool"),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn allow_dangerous_extends_dangerous_set() {
        let p = PermissionPolicy::standard(PermissionMode::DangerFullAccess)
            .allow_dangerous("custom_shell");
        assert_eq!(p.authorize("custom_shell"), PermissionDecision::Allow);
    }

    #[test]
    fn plan_mode_lets_reads_through() {
        let p = PermissionPolicy::standard(PermissionMode::Plan);
        assert_eq!(p.authorize("read_file"), PermissionDecision::Allow);
        assert_eq!(p.authorize("Glob"), PermissionDecision::Allow);
        assert_eq!(p.authorize("Grep"), PermissionDecision::Allow);
    }

    #[test]
    fn plan_mode_dry_runs_writes() {
        let p = PermissionPolicy::standard(PermissionMode::Plan);
        assert_eq!(p.authorize("Edit"), PermissionDecision::AllowDryRun);
        assert_eq!(p.authorize("Write"), PermissionDecision::AllowDryRun);
        assert_eq!(p.authorize("MultiEdit"), PermissionDecision::AllowDryRun);
    }

    #[test]
    fn plan_mode_denies_dangerous_and_unknown() {
        let p = PermissionPolicy::standard(PermissionMode::Plan);
        assert!(matches!(p.authorize("bash"), PermissionDecision::Deny { .. }));
        assert!(matches!(
            p.authorize("totally_new_tool"),
            PermissionDecision::Deny { .. }
        ));
    }
}
