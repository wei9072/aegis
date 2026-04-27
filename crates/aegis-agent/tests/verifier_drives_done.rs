//! AEGIS V3 NEGATIVE-SPACE CONTRACT — verifier drives done
//! ========================================================
//!
//! When the LLM stops emitting `tool_use` (its way of saying
//! "I'm done"), the agent must NOT trust that claim unconditionally.
//! If a verifier is configured, the verifier's verdict is the source
//! of truth.
//!
//! Specifically, `StoppedReason` must distinguish:
//!   - `PlanDoneVerified`         — verifier agreed
//!   - `PlanDoneVerifierRejected` — verifier disagreed ("done" was a lie)
//!   - `PlanDoneNoVerifier`       — no verifier; trust by default, but mark it
//!
//! If anyone collapses these into a single `PlanDone` variant, the
//! agent is back to single-LLM self-evaluation — exactly the failure
//! mode (overly generous self-evaluation, per Anthropic's own
//! observations) that aegis exists to catch.

use aegis_agent::StoppedReason;

#[test]
fn plan_done_variants_distinguish_verifier_state() {
    // These three variants must all exist as distinct values. If a
    // refactor merges any of them, this test fails to compile.
    let _verified = StoppedReason::PlanDoneVerified;
    let _rejected = StoppedReason::PlanDoneVerifierRejected;
    let _no_verifier = StoppedReason::PlanDoneNoVerifier;
}

#[test]
fn plan_done_verified_and_rejected_are_not_equal() {
    assert_ne!(
        StoppedReason::PlanDoneVerified,
        StoppedReason::PlanDoneVerifierRejected
    );
    assert_ne!(
        StoppedReason::PlanDoneVerified,
        StoppedReason::PlanDoneNoVerifier
    );
    assert_ne!(
        StoppedReason::PlanDoneVerifierRejected,
        StoppedReason::PlanDoneNoVerifier
    );
}

/// Compile-time exhaustive coverage of `StoppedReason`. If a new
/// variant is added (especially a bare `PlanDone` that would
/// collapse the verifier discrimination), this match becomes
/// non-exhaustive and the test fails to compile until the new
/// variant is listed here. That forces PR-time reasoning about
/// whether the new variant respects the verifier-overrules-LLM
/// invariant.
#[test]
fn no_generic_plan_done_variant() {
    let observed = [
        StoppedReason::PlanDoneVerified,
        StoppedReason::PlanDoneVerifierRejected,
        StoppedReason::PlanDoneNoVerifier,
        StoppedReason::MaxIterations,
        StoppedReason::CostBudgetExceeded,
        StoppedReason::StalemateDetected,
        StoppedReason::ThrashingDetected,
        StoppedReason::ProviderError(String::new()),
    ];
    for v in observed {
        match v {
            StoppedReason::PlanDoneVerified => {}
            StoppedReason::PlanDoneVerifierRejected => {}
            StoppedReason::PlanDoneNoVerifier => {}
            StoppedReason::MaxIterations => {}
            StoppedReason::CostBudgetExceeded => {}
            StoppedReason::StalemateDetected => {}
            StoppedReason::ThrashingDetected => {}
            StoppedReason::ProviderError(_) => {}
        }
    }
}

/// `AgentTurnResult.task_verdict` must be present (as `Option<TaskVerdict>`).
/// The verifier-driven-done invariant is meaningless if there is no
/// place to put the verdict.
#[test]
fn agent_turn_result_carries_task_verdict_field() {
    // Build a result with no verdict (no-verifier case).
    let result = aegis_agent::AgentTurnResult {
        stopped_reason: StoppedReason::PlanDoneNoVerifier,
        iterations: 1,
        task_verdict: None,
    };
    assert!(result.task_verdict.is_none());

    // Build a result with a verdict (verifier-rejected case).
    let verdict = aegis_decision::TaskVerdict::no_verifier(true, 1);
    let result = aegis_agent::AgentTurnResult {
        stopped_reason: StoppedReason::PlanDoneVerifierRejected,
        iterations: 1,
        task_verdict: Some(verdict),
    };
    assert!(result.task_verdict.is_some());
}
