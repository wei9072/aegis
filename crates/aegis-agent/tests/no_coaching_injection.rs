//! AEGIS V3 NEGATIVE-SPACE CONTRACT — no coaching injection
//! ==========================================================
//!
//! The agent must not have an API that converts verifier verdicts,
//! stalemate signals, or aegis-check verdicts into hint strings
//! that get injected into the next-turn prompt.
//!
//! Verdicts are observation, displayed to the user. The user
//! (not the agent) decides whether to refine the task description
//! and run another turn.
//!
//! This test pins the source-text invariant: forbidden function-
//! and type-name patterns must not appear in the agent crate's
//! `lib.rs`. A future PR that introduces any of them will trip
//! this test before review.
//!
//! Why source-text scanning rather than type-system enforcement:
//! the negative goal is "no such API exists". You cannot express
//! "no function with this shape" purely as a type. A trip-wire
//! list is the cheapest enforcement that surfaces the question.

const FORBIDDEN_SOURCE_TOKENS: &[&str] = &[
    // Function-name patterns: anything that maps verdict → prompt
    // is a coaching injector.
    "fn verdict_to_hint",
    "fn coach_from",
    "fn inject_feedback",
    "fn build_hint_from",
    "fn auto_correction_prompt",
    "fn rewrite_task_with_verdict",
    "fn prompt_from_verdict",
    "fn next_prompt_from_failure",
    // Type-name patterns: structures whose purpose is coaching.
    "AutoHinter",
    "VerdictCoach",
    "FeedbackInjector",
    "RetryPromptBuilder",
];

#[test]
fn lib_source_has_no_coaching_apis() {
    let source = include_str!("../src/lib.rs");
    for forbidden in FORBIDDEN_SOURCE_TOKENS {
        assert!(
            !source.contains(forbidden),
            "aegis-agent lib.rs contains forbidden coaching token {forbidden:?}"
        );
    }
}
