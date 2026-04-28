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

use glob::Pattern;

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

/// One row in `~/.config/aegis/permissions.toml`. Walked in order;
/// first match wins. The bare `tool_glob` pattern is required; the
/// `path_glob` is optional — when present, the rule only fires for
/// tool calls whose `path` field (extracted from the JSON input)
/// matches both patterns. Both patterns use POSIX glob syntax via
/// the `glob` crate (`*`, `?`, `[abc]`, `**` for path crossing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRule {
    pub tool_glob: String,
    pub path_glob: Option<String>,
    pub decision: RuleDecision,
}

/// What a matched rule resolves to. Distinct from `PermissionDecision`
/// because rules can request user prompting — only the runtime knows
/// whether a `PermissionPrompter` is wired in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleDecision {
    Allow,
    Deny,
    /// Defer to the runtime's `PermissionPrompter` (B3.3). When no
    /// prompter is attached, prompt rules collapse to `Deny` — the
    /// safe default for unattended runs.
    Prompt,
    /// Same semantics as `PermissionMode::Plan` but per-rule: write
    /// flows through the predictor without touching disk.
    DryRun,
}

/// Policy decision for one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool is permitted; runtime executes it normally.
    Allow,
    /// Tool is permitted but must NOT execute against disk / shell —
    /// runtime calls the predictor for structural-cost scoring and
    /// returns a synthesized result. Used by `Plan` mode and by
    /// `RuleDecision::DryRun` rule matches.
    AllowDryRun,
    /// Rule matched with `RuleDecision::Prompt` — runtime should
    /// consult its `PermissionPrompter` (or fall through to deny if
    /// none configured). Carries the rule's tool/path info so the
    /// prompter can show the user what's being asked.
    Prompt {
        tool_name: String,
        path: Option<String>,
    },
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
    /// Walked in order before falling back to mode-based decision.
    /// Empty by default; `with_rules()` populates from a TOML config.
    rules: Vec<PermissionRule>,
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
            rules: Vec::new(),
        }
    }

    /// Attach a rule list (typically loaded from
    /// `~/.config/aegis/permissions.toml` by the CLI). Rules are
    /// walked in order on each `authorize_with_input` call and the
    /// first match wins; non-matching tool calls fall through to
    /// the mode-based decision so rules are strictly additive.
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<PermissionRule>) -> Self {
        self.rules = rules;
        self
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

    /// Like `authorize`, but consults the `rules` list first. The
    /// `input` JSON is parsed for a `path` field — rules with
    /// `path_glob` only match when both the tool glob AND the path
    /// glob succeed. Tool calls with no `path` field skip rules that
    /// require one.
    pub fn authorize_with_input(&self, tool_name: &str, input: &str) -> PermissionDecision {
        let path = parse_path_field(input);
        for rule in &self.rules {
            if !glob_matches(&rule.tool_glob, tool_name) {
                continue;
            }
            if let Some(path_glob) = &rule.path_glob {
                let Some(p) = path.as_deref() else { continue };
                if !glob_matches(path_glob, p) {
                    continue;
                }
            }
            return decision_for(rule.decision, tool_name, path);
        }
        // No rule matched — fall through to the mode-based decision.
        self.authorize(tool_name)
    }
}

fn decision_for(
    rule: RuleDecision,
    tool_name: &str,
    path: Option<String>,
) -> PermissionDecision {
    match rule {
        RuleDecision::Allow => PermissionDecision::Allow,
        RuleDecision::Deny => PermissionDecision::Deny {
            reason: format!(
                "permission denied by rule: tool {tool_name:?}{}",
                path.as_deref()
                    .map(|p| format!(" path {p:?}"))
                    .unwrap_or_default()
            ),
        },
        RuleDecision::DryRun => PermissionDecision::AllowDryRun,
        RuleDecision::Prompt => PermissionDecision::Prompt {
            tool_name: tool_name.to_string(),
            path,
        },
    }
}

/// Parse the `path` field out of a JSON tool-input string. Used by
/// `authorize_with_input` so rules with `path_glob` can match. Tools
/// whose schemas don't expose a `path` field (e.g. `Bash`) get
/// `None` — rules with `path_glob` then skip them, falling through
/// to whatever rule matches by tool name only.
fn parse_path_field(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    value
        .get("path")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    Pattern::new(pattern)
        .map(|p| p.matches(value))
        .unwrap_or(false)
}

// ---------- B3.3: PermissionPrompter trait ----------

/// Outcome of a `PermissionPrompter::ask` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    /// Allow this single call only.
    AllowOnce,
    /// Allow this AND all future calls matching the same
    /// (tool, path) pair within the session.
    AllowAlways,
    /// Deny this single call.
    DenyOnce,
    /// Deny this AND all future calls matching the same pair.
    DenyAlways,
}

/// User-facing prompter — invoked by the runtime when a
/// `PermissionDecision::Prompt` arrives. Implementations decide how
/// to ask (REPL TTY, GUI dialog, web hook). The runtime never
/// blocks on a prompter that doesn't exist — when no prompter is
/// configured, prompt decisions collapse to deny, matching the
/// safe default for unattended runs.
pub trait PermissionPrompter: Send {
    /// `tool_name` and `path` come straight from the matched rule.
    /// `reason` describes the rule (e.g. "rule #2: tool=Bash
    /// path=*"). Implementations should show all three to the user.
    fn ask(&mut self, tool_name: &str, path: Option<&str>, reason: &str) -> PromptOutcome;
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

    // ---------- B3.2: rule-based authorization ----------

    fn rule(tool: &str, path: Option<&str>, decision: RuleDecision) -> PermissionRule {
        PermissionRule {
            tool_glob: tool.to_string(),
            path_glob: path.map(String::from),
            decision,
        }
    }

    #[test]
    fn no_rules_falls_back_to_mode_decision() {
        let p = PermissionPolicy::standard(PermissionMode::ReadOnly);
        let v = p.authorize_with_input("Edit", r#"{"path":"x.py"}"#);
        assert!(matches!(v, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn rule_matching_tool_and_path_grants_allow() {
        let p = PermissionPolicy::standard(PermissionMode::ReadOnly).with_rules(vec![rule(
            "Edit",
            Some("src/**/*.rs"),
            RuleDecision::Allow,
        )]);
        let v = p.authorize_with_input("Edit", r#"{"path":"src/foo/bar.rs"}"#);
        assert_eq!(v, PermissionDecision::Allow);
    }

    #[test]
    fn rule_path_glob_skipped_when_input_lacks_path_field() {
        // Bash inputs typically have no "path" field — a path-bound
        // rule should NOT match, falling through to mode decision.
        let p = PermissionPolicy::standard(PermissionMode::DangerFullAccess).with_rules(vec![
            rule("*", Some("src/**"), RuleDecision::Deny),
        ]);
        let v = p.authorize_with_input("Bash", r#"{"command":"ls"}"#);
        assert_eq!(v, PermissionDecision::Allow); // mode allows; no path → rule skipped
    }

    #[test]
    fn first_matching_rule_wins() {
        let p = PermissionPolicy::standard(PermissionMode::ReadOnly).with_rules(vec![
            rule("Edit", Some("src/**"), RuleDecision::Allow),
            rule("Edit", Some("**"), RuleDecision::Deny),
        ]);
        let v = p.authorize_with_input("Edit", r#"{"path":"src/foo.rs"}"#);
        assert_eq!(v, PermissionDecision::Allow);
    }

    #[test]
    fn deny_rule_overrides_workspace_write_mode() {
        let p = PermissionPolicy::standard(PermissionMode::WorkspaceWrite).with_rules(vec![
            rule("Edit", Some("vendor/**"), RuleDecision::Deny),
        ]);
        let v = p.authorize_with_input("Edit", r#"{"path":"vendor/dep.rs"}"#);
        assert!(matches!(v, PermissionDecision::Deny { .. }));
        // Same tool, different path → falls through to mode → Allow.
        let v2 = p.authorize_with_input("Edit", r#"{"path":"src/main.rs"}"#);
        assert_eq!(v2, PermissionDecision::Allow);
    }

    #[test]
    fn dry_run_rule_yields_allow_dry_run_decision() {
        let p = PermissionPolicy::standard(PermissionMode::WorkspaceWrite).with_rules(vec![
            rule("Edit", Some("**/*.toml"), RuleDecision::DryRun),
        ]);
        let v = p.authorize_with_input("Edit", r#"{"path":"Cargo.toml"}"#);
        assert_eq!(v, PermissionDecision::AllowDryRun);
    }

    #[test]
    fn prompt_rule_returns_prompt_decision_with_context() {
        let p = PermissionPolicy::standard(PermissionMode::ReadOnly).with_rules(vec![
            rule("Bash", None, RuleDecision::Prompt),
        ]);
        let v = p.authorize_with_input("Bash", r#"{"command":"ls"}"#);
        match v {
            PermissionDecision::Prompt { tool_name, path } => {
                assert_eq!(tool_name, "Bash");
                assert_eq!(path, None);
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn tool_glob_wildcard_matches_anything() {
        let p = PermissionPolicy::standard(PermissionMode::WorkspaceWrite).with_rules(vec![
            rule("*", Some("secrets/*"), RuleDecision::Deny),
        ]);
        let v = p.authorize_with_input("Edit", r#"{"path":"secrets/key.txt"}"#);
        assert!(matches!(v, PermissionDecision::Deny { .. }));
        let v2 = p.authorize_with_input("Read", r#"{"path":"secrets/key.txt"}"#);
        assert!(matches!(v2, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn parse_path_field_handles_missing_or_invalid_json() {
        assert_eq!(parse_path_field(r#"{"path":"x"}"#), Some("x".into()));
        assert_eq!(parse_path_field(r#"{"command":"ls"}"#), None);
        assert_eq!(parse_path_field("not json at all"), None);
        assert_eq!(parse_path_field(""), None);
    }
}
