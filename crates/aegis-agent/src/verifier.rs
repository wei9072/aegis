//! Task-level verifier integration — V3.3 differentiation #3.
//!
//! When the LLM stops emitting `tool_use` (its way of saying "done"),
//! the conversation runtime asks the configured verifier whether the
//! task is actually complete. The verifier's verdict overrides the
//! LLM's claim.
//!
//! This is the structural defence against
//! "overly-generous self-evaluation" (Anthropic's phrase). LLMs are
//! known to declare done while leaving the work broken — verifiers
//! check by exercising an external source of truth (running tests,
//! invoking the build, querying structural targets) that the LLM
//! cannot fake.
//!
//! Negative-space discipline:
//! - Verifier verdicts go to `AgentTurnResult.task_verdict` for the
//!   user to read.
//! - The verdict is NEVER turned into a hint string and prepended
//!   to the next turn's prompt — that's the auto-retry / coaching
//!   pattern, structurally banned by `tests/no_coaching_injection.rs`.
//!
//! Trait + concrete impls (TestVerifier / BuildVerifier /
//! ShellVerifier) live here so the agent can ship batteries-included
//! defences for the most common project shapes without a network
//! round-trip.

use std::path::{Path, PathBuf};
use std::process::Command;

use aegis_decision::VerifierResult;

/// Verifier contract for the agent. Distinct from
/// `aegis_decision::TaskVerifier` (which takes an IterationEvent
/// trace) — this one takes only the workspace path because the
/// agent doesn't speak in IterationEvents.
pub trait AgentTaskVerifier: Send + Sync {
    /// Run the verifier against the given workspace. Returns
    /// `VerifierResult { passed, rationale, evidence }` — the
    /// runtime turns `passed: true` into `StoppedReason::PlanDoneVerified`
    /// and `false` into `PlanDoneVerifierRejected`.
    fn verify(&self, workspace: &Path) -> VerifierResult;
}

// ---------- ShellVerifier ----------

/// Run an arbitrary shell command. Verdict = `exit_code == 0`.
/// Captures stderr into the rationale on failure for human inspection
/// (NOT fed to the LLM — see `no_coaching_injection.rs`).
pub struct ShellVerifier {
    pub program: String,
    pub args: Vec<String>,
    /// Optional override for the working directory; defaults to
    /// the workspace passed to `verify`.
    pub working_dir: Option<PathBuf>,
}

impl ShellVerifier {
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

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }
}

impl AgentTaskVerifier for ShellVerifier {
    fn verify(&self, workspace: &Path) -> VerifierResult {
        let cwd = self
            .working_dir
            .clone()
            .unwrap_or_else(|| workspace.to_path_buf());
        let output = Command::new(&self.program)
            .args(&self.args)
            .current_dir(&cwd)
            .output();
        match output {
            Ok(out) => {
                let passed = out.status.success();
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let rationale = if passed {
                    format!("{} {} ⇒ exit 0", self.program, self.args.join(" "))
                } else {
                    format!(
                        "{} {} ⇒ exit {} — stderr: {} — stdout: {}",
                        self.program,
                        self.args.join(" "),
                        out.status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "?".into()),
                        stderr.trim(),
                        stdout.trim()
                    )
                };
                VerifierResult::new(passed).with_rationale(rationale)
            }
            Err(e) => VerifierResult::new(false)
                .with_rationale(format!("ShellVerifier failed to spawn: {e}")),
        }
    }
}

// ---------- TestVerifier ----------

/// Auto-detect the project type and run its test suite.
/// Detection precedence (first match wins): Cargo.toml → cargo test,
/// pyproject.toml → pytest, package.json → npm test, go.mod → go test.
pub struct TestVerifier {
    inner: ShellVerifier,
}

impl TestVerifier {
    /// Try to auto-detect a runnable test command for the given
    /// workspace. Returns `None` if no recognised project marker
    /// is present.
    #[must_use]
    pub fn auto_detect(workspace: &Path) -> Option<Self> {
        if workspace.join("Cargo.toml").exists() {
            return Some(Self {
                inner: ShellVerifier::new("cargo").arg("test").arg("--quiet"),
            });
        }
        if workspace.join("pyproject.toml").exists() || workspace.join("setup.py").exists() {
            return Some(Self {
                inner: ShellVerifier::new("pytest").arg("-q"),
            });
        }
        if workspace.join("package.json").exists() {
            return Some(Self {
                inner: ShellVerifier::new("npm").arg("test").arg("--silent"),
            });
        }
        if workspace.join("go.mod").exists() {
            return Some(Self {
                inner: ShellVerifier::new("go").arg("test").arg("./..."),
            });
        }
        None
    }
}

impl AgentTaskVerifier for TestVerifier {
    fn verify(&self, workspace: &Path) -> VerifierResult {
        self.inner.verify(workspace)
    }
}

// ---------- BuildVerifier ----------

/// Auto-detect the project type and run its compile / type-check.
/// Lighter than tests — catches syntax / type / import errors
/// without exercising runtime behaviour.
pub struct BuildVerifier {
    inner: ShellVerifier,
}

impl BuildVerifier {
    #[must_use]
    pub fn auto_detect(workspace: &Path) -> Option<Self> {
        if workspace.join("Cargo.toml").exists() {
            return Some(Self {
                inner: ShellVerifier::new("cargo").arg("check").arg("--quiet"),
            });
        }
        if workspace.join("tsconfig.json").exists() {
            return Some(Self {
                inner: ShellVerifier::new("npx").arg("tsc").arg("--noEmit"),
            });
        }
        if workspace.join("pyproject.toml").exists() && workspace.join("mypy.ini").exists() {
            return Some(Self {
                inner: ShellVerifier::new("mypy").arg("."),
            });
        }
        if workspace.join("go.mod").exists() {
            return Some(Self {
                inner: ShellVerifier::new("go").arg("build").arg("./..."),
            });
        }
        None
    }
}

impl AgentTaskVerifier for BuildVerifier {
    fn verify(&self, workspace: &Path) -> VerifierResult {
        self.inner.verify(workspace)
    }
}

// ---------- CompositeVerifier ----------

/// Run multiple verifiers in order. Verdict is `passed` only if ALL
/// pass. The rationale concatenates each child's rationale.
pub struct CompositeVerifier {
    pub verifiers: Vec<Box<dyn AgentTaskVerifier>>,
}

impl CompositeVerifier {
    #[must_use]
    pub fn new(verifiers: Vec<Box<dyn AgentTaskVerifier>>) -> Self {
        Self { verifiers }
    }
}

impl AgentTaskVerifier for CompositeVerifier {
    fn verify(&self, workspace: &Path) -> VerifierResult {
        let mut all_pass = true;
        let mut rationales = Vec::new();
        for v in &self.verifiers {
            let r = v.verify(workspace);
            if !r.passed {
                all_pass = false;
            }
            rationales.push(r.rationale);
        }
        VerifierResult::new(all_pass).with_rationale(rationales.join("\n---\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_verifier_true_passes() {
        let v = ShellVerifier::new("true");
        let result = v.verify(Path::new("."));
        assert!(result.passed);
    }

    #[test]
    fn shell_verifier_false_fails() {
        let v = ShellVerifier::new("false");
        let result = v.verify(Path::new("."));
        assert!(!result.passed);
        assert!(result.rationale.contains("exit 1"));
    }

    #[test]
    fn shell_verifier_missing_program_fails_gracefully() {
        let v = ShellVerifier::new("definitely-not-a-real-program-12345");
        let result = v.verify(Path::new("."));
        assert!(!result.passed);
        assert!(result.rationale.contains("ShellVerifier failed to spawn"));
    }

    #[test]
    fn test_verifier_auto_detect_returns_none_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(TestVerifier::auto_detect(dir.path()).is_none());
    }

    #[test]
    fn test_verifier_auto_detect_picks_cargo_when_cargo_toml_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert!(TestVerifier::auto_detect(dir.path()).is_some());
    }

    #[test]
    fn composite_verifier_passes_only_if_all_pass() {
        let pass = ShellVerifier::new("true");
        let fail = ShellVerifier::new("false");
        let composite = CompositeVerifier::new(vec![Box::new(pass), Box::new(fail)]);
        let result = composite.verify(Path::new("."));
        assert!(!result.passed);
    }

    #[test]
    fn composite_verifier_all_pass_passes() {
        let pass1 = ShellVerifier::new("true");
        let pass2 = ShellVerifier::new("true");
        let composite = CompositeVerifier::new(vec![Box::new(pass1), Box::new(pass2)]);
        let result = composite.verify(Path::new("."));
        assert!(result.passed);
    }
}
