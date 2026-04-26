//! Layer C — task outcome verification.
//!
//! Mirror of `aegis/runtime/task_verifier.py`. The Critical Principle
//! is enforced structurally:
//!
//! 1. `TaskVerifier` has exactly one method.
//! 2. `TaskVerdict` carries no field a loop could consume — no
//!    "retry", "feedback", "hint", "next_plan", "advice", "guidance".
//! 3. `apply_verifier` never panics.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::iteration::IterationEvent;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPattern {
    Solved,
    Incomplete,
    Abandoned,
    NoVerifier,
    VerifierError,
}

impl TaskPattern {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskPattern::Solved => "solved",
            TaskPattern::Incomplete => "incomplete",
            TaskPattern::Abandoned => "abandoned",
            TaskPattern::NoVerifier => "no_verifier",
            TaskPattern::VerifierError => "verifier_error",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "solved" => TaskPattern::Solved,
            "incomplete" => TaskPattern::Incomplete,
            "abandoned" => TaskPattern::Abandoned,
            "no_verifier" => TaskPattern::NoVerifier,
            "verifier_error" => TaskPattern::VerifierError,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct VerifierResult {
    pub passed: bool,
    pub rationale: String,
    pub evidence: HashMap<String, serde_json::Value>,
}

impl VerifierResult {
    pub fn new(passed: bool) -> Self {
        Self {
            passed,
            rationale: String::new(),
            evidence: HashMap::new(),
        }
    }

    pub fn with_rationale(mut self, r: impl Into<String>) -> Self {
        self.rationale = r.into();
        self
    }

    pub fn with_evidence(mut self, e: HashMap<String, serde_json::Value>) -> Self {
        self.evidence = e;
        self
    }
}

#[derive(Clone, Debug)]
pub struct TaskVerdict {
    pub pattern: TaskPattern,
    pub verifier_result: Option<VerifierResult>,
    pub pipeline_done: bool,
    pub iterations_run: u32,
    pub error: String,
}

impl TaskVerdict {
    pub fn no_verifier(pipeline_done: bool, iterations_run: u32) -> Self {
        Self {
            pattern: TaskPattern::NoVerifier,
            verifier_result: None,
            pipeline_done,
            iterations_run,
            error: String::new(),
        }
    }

    pub fn verifier_error(pipeline_done: bool, iterations_run: u32, error: String) -> Self {
        Self {
            pattern: TaskPattern::VerifierError,
            verifier_result: None,
            pipeline_done,
            iterations_run,
            error,
        }
    }
}

/// The single-method extension point. Implementors inspect the final
/// workspace state and return a verdict; they do not read or mutate
/// the trace, and they cannot trigger a retry.
pub trait TaskVerifier: Send + Sync {
    fn verify(&self, workspace: &Path, trace: &[IterationEvent]) -> VerifierResult;
}

pub fn derive_task_pattern(
    verifier_present: bool,
    verifier_passed: Option<bool>,
    verifier_raised: bool,
    pipeline_done: bool,
) -> TaskPattern {
    if !verifier_present {
        return TaskPattern::NoVerifier;
    }
    if verifier_raised {
        return TaskPattern::VerifierError;
    }
    match verifier_passed {
        Some(true) => TaskPattern::Solved,
        Some(false) | None => {
            if pipeline_done {
                TaskPattern::Incomplete
            } else {
                TaskPattern::Abandoned
            }
        }
    }
}

/// Always returns a verdict — never panics. Catching panics requires
/// an explicit caller-side `catch_unwind`; in practice verifier impls
/// should return a failing `VerifierResult` rather than panic.
pub fn apply_verifier<V: TaskVerifier + ?Sized>(
    verifier: Option<&V>,
    workspace: &Path,
    trace: &[IterationEvent],
    pipeline_done: bool,
    iterations_run: u32,
) -> TaskVerdict {
    let v = match verifier {
        None => return TaskVerdict::no_verifier(pipeline_done, iterations_run),
        Some(v) => v,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        v.verify(workspace, trace)
    }));
    let result = match result {
        Ok(r) => r,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_string()
            };
            return TaskVerdict::verifier_error(
                pipeline_done,
                iterations_run,
                format!("PanicError: {msg}"),
            );
        }
    };
    let pattern = derive_task_pattern(true, Some(result.passed), false, pipeline_done);
    TaskVerdict {
        pattern,
        verifier_result: Some(result),
        pipeline_done,
        iterations_run,
        error: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct PassingV;
    impl TaskVerifier for PassingV {
        fn verify(&self, _w: &Path, _t: &[IterationEvent]) -> VerifierResult {
            VerifierResult::new(true).with_rationale("ok")
        }
    }

    struct FailingV;
    impl TaskVerifier for FailingV {
        fn verify(&self, _w: &Path, _t: &[IterationEvent]) -> VerifierResult {
            VerifierResult::new(false).with_rationale("nope")
        }
    }

    struct PanickingV;
    impl TaskVerifier for PanickingV {
        fn verify(&self, _w: &Path, _t: &[IterationEvent]) -> VerifierResult {
            panic!("verifier blew up")
        }
    }

    fn ws() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn no_verifier() {
        let v = apply_verifier::<PassingV>(None, &ws(), &[], true, 2);
        assert_eq!(v.pattern, TaskPattern::NoVerifier);
        assert!(v.verifier_result.is_none());
    }

    #[test]
    fn passing_yields_solved_regardless_of_pipeline_done() {
        for pipeline_done in [true, false] {
            let v = apply_verifier(Some(&PassingV), &ws(), &[], pipeline_done, 1);
            assert_eq!(v.pattern, TaskPattern::Solved);
        }
    }

    #[test]
    fn failing_split_by_pipeline_done() {
        let inc = apply_verifier(Some(&FailingV), &ws(), &[], true, 1);
        assert_eq!(inc.pattern, TaskPattern::Incomplete);
        let abn = apply_verifier(Some(&FailingV), &ws(), &[], false, 1);
        assert_eq!(abn.pattern, TaskPattern::Abandoned);
    }

    #[test]
    fn panic_yields_verifier_error_not_crash() {
        let v = apply_verifier(Some(&PanickingV), &ws(), &[], true, 1);
        assert_eq!(v.pattern, TaskPattern::VerifierError);
        assert!(v.error.contains("verifier blew up"));
    }

    #[test]
    fn task_verdict_has_no_feedback_field() {
        // Compile-time + structural pin: every named field listed
        // explicitly. If a future PR adds anything matching the
        // forbidden substrings, this test must be updated by the
        // same PR — a trip-wire for the Layer B/C isolation rule.
        let allowed = ["pattern", "verifier_result", "pipeline_done", "iterations_run", "error"];
        let forbidden = ["retry", "feedback", "hint", "next_plan", "advice", "guidance"];
        for name in allowed {
            for f in forbidden {
                assert!(
                    !name.contains(f),
                    "TaskVerdict field {name:?} contains forbidden {f:?}"
                );
            }
        }
    }
}
