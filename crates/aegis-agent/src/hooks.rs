//! PreToolUse / PostToolUse shell-command hooks — Claude Code parity.
//!
//! Mirrors the protocol Claude Code's `.claude/settings.json` uses:
//! a hook is a shell command that receives a JSON payload on stdin
//! and may write a response JSON to stdout. Exit code 0 = continue;
//! exit code 2 = block (stderr is the reason).
//!
//! V3.6 keeps it minimal — just exit-code behaviour, no JSON-merge
//! "updated_input" feature yet (claw-code's HookRunner has more, can
//! be added when a real consumer needs it).
//!
//! Negative-space discipline: a PreToolUse hook can BLOCK
//! (the runtime synthesises a tool_result is_error=true with the
//! hook's stderr as the reason). It cannot REWRITE the prompt or
//! inject coaching. The runtime never reads more than exit-code +
//! stderr from the hook process.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::predict::{PreToolUsePredictor, PredictVerdict};

/// One hook = one shell command with optional args + working dir.
#[derive(Clone, Debug)]
pub struct ShellHook {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<std::path::PathBuf>,
}

impl ShellHook {
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn working_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Run the hook with `stdin_payload` piped to its stdin. Returns
    /// `(exit_code, stderr_text)`. Hooks that fail to spawn yield
    /// (-1, "<spawn error>").
    pub fn run(&self, stdin_payload: &str) -> (i32, String) {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return (-1, format!("hook spawn error: {e}")),
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(stdin_payload.as_bytes());
        }
        let output = match child.wait_with_output() {
            Ok(o) => o,
            Err(e) => return (-1, format!("hook wait error: {e}")),
        };
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        (code, stderr)
    }
}

/// PreToolUse hook predictor: hook exit 2 → Block; anything else →
/// Allow. Multiple hooks evaluate in order; first Block wins.
pub struct PreToolUseHookPredictor {
    pub hooks: Vec<ShellHook>,
}

impl PreToolUseHookPredictor {
    #[must_use]
    pub fn new(hooks: Vec<ShellHook>) -> Self {
        Self { hooks }
    }
}

impl PreToolUsePredictor for PreToolUseHookPredictor {
    fn predict(&mut self, tool_name: &str, input: &str) -> PredictVerdict {
        // Mirror Claude Code's payload shape so existing user hook
        // scripts work unchanged.
        let payload = serde_json::json!({
            "tool_name": tool_name,
            "tool_input": serde_json::from_str::<serde_json::Value>(input)
                .unwrap_or(serde_json::Value::String(input.to_string())),
        })
        .to_string();
        for hook in &self.hooks {
            let (code, stderr) = hook.run(&payload);
            if code == 2 {
                let reason = stderr.trim();
                let reason = if reason.is_empty() {
                    format!("PreToolUse hook {} blocked the call", hook.program)
                } else {
                    reason.to_string()
                };
                return PredictVerdict::Block { reason };
            }
        }
        PredictVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_hook_true_returns_zero_and_empty_stderr() {
        let h = ShellHook::new("true");
        let (code, _) = h.run("");
        assert_eq!(code, 0);
    }

    #[test]
    fn shell_hook_false_returns_one() {
        let h = ShellHook::new("false");
        let (code, _) = h.run("");
        assert_eq!(code, 1);
    }

    #[test]
    fn pre_tool_use_hook_blocks_when_hook_exits_2() {
        // sh -c 'exit 2' simulates a hook that vetoes
        let hook = ShellHook::new("sh").arg("-c").arg("exit 2");
        let mut p = PreToolUseHookPredictor::new(vec![hook]);
        match p.predict("anything", "{}") {
            PredictVerdict::Block { .. } => {}
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn pre_tool_use_hook_allows_when_hook_exits_zero() {
        let hook = ShellHook::new("true");
        let mut p = PreToolUseHookPredictor::new(vec![hook]);
        assert_eq!(p.predict("anything", "{}"), PredictVerdict::Allow);
    }

    #[test]
    fn pre_tool_use_hook_block_carries_stderr_as_reason() {
        let hook = ShellHook::new("sh")
            .arg("-c")
            .arg("echo 'bad input' >&2 ; exit 2");
        let mut p = PreToolUseHookPredictor::new(vec![hook]);
        match p.predict("write_file", r#"{"x":1}"#) {
            PredictVerdict::Block { reason } => {
                assert!(reason.contains("bad input"));
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }
}
