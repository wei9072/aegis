//! V3.4 — verifier integration tests.
//!
//! Drives ConversationRuntime end-to-end with a verifier wired in.
//! Verifies that:
//!   - No verifier configured → PlanDoneNoVerifier (current default)
//!   - Verifier passes → PlanDoneVerified + task_verdict = SOLVED
//!   - Verifier fails → PlanDoneVerifierRejected + task_verdict = INCOMPLETE
//!   - Verifier verdict is NEVER used to retry the turn
//!   - rationale is captured but not fed back to the LLM

use aegis_agent::testing::{ScriptedApiClient, ScriptedToolExecutor};
use aegis_agent::verifier::{AgentTaskVerifier, ShellVerifier};
use aegis_agent::{
    AgentConfig, ConversationRuntime, Session, StoppedReason,
};
use aegis_decision::{TaskPattern, VerifierResult};
use std::path::Path;

fn cfg() -> AgentConfig {
    AgentConfig {
        max_iterations_per_turn: 5,
        session_cost_budget: None,
        workspace_root: Some(std::env::temp_dir()),
    }
}

struct AlwaysPassVerifier;
impl AgentTaskVerifier for AlwaysPassVerifier {
    fn verify(&self, _: &Path) -> VerifierResult {
        VerifierResult::new(true).with_rationale("test always passes")
    }
}

struct AlwaysFailVerifier;
impl AgentTaskVerifier for AlwaysFailVerifier {
    fn verify(&self, _: &Path) -> VerifierResult {
        VerifierResult::new(false).with_rationale("test always fails")
    }
}

#[test]
fn no_verifier_yields_plan_done_no_verifier_unchanged() {
    let api = ScriptedApiClient::new().push_text_then_done("done");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg());

    let result = rt.run_turn("hi");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneNoVerifier);
    assert!(result.task_verdict.is_none());
}

#[test]
fn passing_verifier_yields_plan_done_verified() {
    let api = ScriptedApiClient::new().push_text_then_done("done");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_verifier(Box::new(AlwaysPassVerifier));

    let result = rt.run_turn("hi");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneVerified);

    let verdict = result.task_verdict.expect("verdict must be set");
    assert_eq!(verdict.pattern, TaskPattern::Solved);
    assert_eq!(verdict.iterations_run, 1);
    assert!(verdict.pipeline_done);
}

#[test]
fn failing_verifier_yields_plan_done_verifier_rejected() {
    let api = ScriptedApiClient::new().push_text_then_done("done");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_verifier(Box::new(AlwaysFailVerifier));

    let result = rt.run_turn("hi");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneVerifierRejected);

    let verdict = result.task_verdict.expect("verdict must be set");
    assert_eq!(verdict.pattern, TaskPattern::Incomplete);
    let inner = verdict.verifier_result.expect("verifier result must be set");
    assert!(!inner.passed);
    assert!(inner.rationale.contains("always fails"));
}

#[test]
fn failing_verifier_does_not_trigger_extra_iterations() {
    // Critical: verifier failure must NOT cause the runtime to
    // re-prompt the LLM "the verifier failed, please fix" — that's
    // auto-retry / coaching injection, structurally banned.
    let api = ScriptedApiClient::new().push_text_then_done("first turn");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_verifier(Box::new(AlwaysFailVerifier));

    let result = rt.run_turn("hi");
    assert_eq!(result.iterations, 1);
    // Session must contain exactly user + assistant — no follow-up
    // prompt with verifier rationale prepended.
    assert_eq!(rt.session().messages.len(), 2);
}

// ---------- ShellVerifier wired into conversation ----------

#[test]
fn shell_verifier_true_via_conversation_runtime_passes() {
    let api = ScriptedApiClient::new().push_text_then_done("done");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_verifier(Box::new(ShellVerifier::new("true")));

    let result = rt.run_turn("hi");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneVerified);
}

#[test]
fn shell_verifier_false_via_conversation_runtime_rejects() {
    let api = ScriptedApiClient::new().push_text_then_done("done");
    let tools = ScriptedToolExecutor::new();
    let mut rt = ConversationRuntime::new(Session::new(), api, tools, vec![], vec![], cfg())
        .with_verifier(Box::new(ShellVerifier::new("false")));

    let result = rt.run_turn("hi");
    assert_eq!(result.stopped_reason, StoppedReason::PlanDoneVerifierRejected);
}
