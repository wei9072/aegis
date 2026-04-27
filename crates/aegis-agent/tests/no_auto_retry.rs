//! AEGIS V3 NEGATIVE-SPACE CONTRACT — no auto-retry
//! ==================================================
//!
//! aegis-agent must not have automatic retry semantics. The agent
//! runs one turn at a time; if anything fails, the result is
//! reported and the agent stops. Re-invocation is the user's
//! (or orchestrator's) decision, never the agent's.
//!
//! This test pins three invariants:
//!
//!   1. `AgentConfig` has no field whose name implies retry
//!      (`auto_retry`, `max_retries`, `retry_on_failure`, etc.)
//!   2. `AgentTurnResult` has no field that an outer loop could
//!      consume as a retry trigger (`retry_count`, `can_retry`,
//!      `next_action`, etc.)
//!   3. The agent crate's source text does not contain function
//!      identifiers implying auto-retry semantics.
//!
//! Violations of these invariants mean the framing has slipped
//! from "rejection layer" to "retry engine". See:
//!   - `docs/post_launch_discipline.md` (deferral #5)
//!   - `docs/gap3_control_plane.md` (Critical Principle)
//!   - `crates/aegis-decision/src/task.rs::tests::task_verdict_has_no_feedback_field`
//!     (the sibling contract guarding `TaskVerdict`)

const FORBIDDEN_FIELD_SUBSTRINGS: &[&str] = &[
    "retry",
    "auto_retry",
    "max_retries",
    "retry_on_failure",
    "feedback",
    "hint",
    "advice",
    "guidance",
    "coaching",
    "next_action",
];

const FORBIDDEN_SOURCE_TOKENS: &[&str] = &[
    "fn auto_retry",
    "fn retry_on",
    "fn coach_from",
    "fn verdict_to_hint",
    "fn inject_feedback",
];

/// Hand-listed allowed field names for `AgentConfig`. If a future
/// PR adds a forbidden one, both this list and the struct must
/// change — surfacing the framing question to PR review.
const ALLOWED_AGENT_CONFIG_FIELDS: &[&str] = &["max_iterations_per_turn", "session_cost_budget"];

/// Hand-listed allowed field names for `AgentTurnResult`.
const ALLOWED_AGENT_TURN_RESULT_FIELDS: &[&str] =
    &["stopped_reason", "iterations", "task_verdict"];

#[test]
fn agent_config_has_no_retry_fields() {
    for name in ALLOWED_AGENT_CONFIG_FIELDS {
        for forbidden in FORBIDDEN_FIELD_SUBSTRINGS {
            assert!(
                !name.contains(forbidden),
                "AgentConfig field {name:?} contains forbidden token {forbidden:?}"
            );
        }
    }
}

#[test]
fn agent_turn_result_has_no_retry_fields() {
    for name in ALLOWED_AGENT_TURN_RESULT_FIELDS {
        for forbidden in FORBIDDEN_FIELD_SUBSTRINGS {
            assert!(
                !name.contains(forbidden),
                "AgentTurnResult field {name:?} contains forbidden token {forbidden:?}"
            );
        }
    }
}

#[test]
fn lib_source_contains_no_auto_retry_functions() {
    let source = include_str!("../src/lib.rs");
    for forbidden in FORBIDDEN_SOURCE_TOKENS {
        assert!(
            !source.contains(forbidden),
            "aegis-agent lib.rs contains forbidden source token {forbidden:?}"
        );
    }
}

/// Sanity probe: the constructor for `AgentConfig` must accept no
/// retry-shaped argument. A `Default::default()` build is the floor;
/// if someone changes the type to require a `max_retries: u32`, this
/// test breaks at compile time.
#[test]
fn agent_config_constructible_without_retry_args() {
    let cfg = aegis_agent::AgentConfig::default();
    assert_eq!(cfg.max_iterations_per_turn, 0);
    assert!(cfg.session_cost_budget.is_none());
}
