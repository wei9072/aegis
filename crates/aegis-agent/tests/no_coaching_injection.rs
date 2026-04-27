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
fn crate_source_has_no_coaching_apis() {
    let source = collect_all_crate_source();
    for forbidden in FORBIDDEN_SOURCE_TOKENS {
        assert!(
            !source.contains(forbidden),
            "aegis-agent src/ contains forbidden coaching token {forbidden:?}"
        );
    }
}

/// Walk the crate's `src/` directory and concatenate every `.rs`
/// file's contents. Used by source-text contract scans so adding
/// a new module never silently bypasses the trip-wire.
fn collect_all_crate_source() -> String {
    let mut out = String::new();
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    walk_rs(&src_dir, &mut out);
    out
}

fn walk_rs(dir: &std::path::Path, out: &mut String) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                out.push_str(&content);
                out.push('\n');
            }
        }
    }
}
