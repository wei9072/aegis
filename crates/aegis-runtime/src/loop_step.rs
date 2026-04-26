//! Per-iteration loop decision logic — the pure functional core of
//! `Pipeline._run_loop`.
//!
//! The loop body in `aegis/runtime/pipeline.py` (and its Rust port
//! in `aegis-pyshim::pipeline`) performs three side-effecting calls
//! per iteration: planner → validator → executor. Sandwiched between
//! those is the *decision* about whether to terminate (stalemate /
//! thrashing / max-iters) and what label this iteration's
//! IterationEvent should carry.
//!
//! That decision is pure — it depends only on the history of
//! per-iteration `value_totals` snapshots + `regressed` booleans +
//! the current values. Extracting it here lets:
//!
//! 1. `cargo test` exhaustively pin the decision branches without
//!    standing up a fake Planner / Executor.
//! 2. Both the Python loop (today) and the Rust loop (post V1.3
//!    full) share the exact same code path — no chance of subtle
//!    behaviour drift between the two implementations during the
//!    transition.
//!
//! The Python `_step` closure in `pipeline.py` predates this module;
//! it's now equivalent to building a `LoopState`, calling
//! `step_decision`, and using the returned `StepDecision` to pick
//! the IterationEvent's `stalemate_detected` / `thrashing_detected`
//! flags + decide whether to break out of the loop.

use std::collections::BTreeMap;

use crate::sequence::{is_plan_repeat_stalemate, is_state_stalemate, is_thrashing};

/// Why the loop should stop. The detector reasons are kept as
/// short stable strings so a Rust-side caller and the Python
/// `PipelineResult.error` field share identical language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminateReason {
    Thrashing,
    StateStalemate,
    PlanRepeatStalemate,
}

impl TerminateReason {
    /// Human-readable, mirrors the V0.x Python `_step` closure
    /// strings byte-for-byte (used by tests + downstream tooling
    /// that pattern-matches on `result.error`).
    pub fn as_message(&self) -> String {
        match self {
            TerminateReason::Thrashing => format!(
                "thrashing detected — {} consecutive regression rollbacks; \
                 further iterations would burn budget",
                THRASHING_THRESHOLD
            ),
            TerminateReason::StateStalemate => format!(
                "state stalemate — signal_value_totals unchanged for {} \
                 iters; loop is making no progress",
                STATE_STALEMATE_THRESHOLD
            ),
            TerminateReason::PlanRepeatStalemate => {
                "stalemate — planner repeated identical plan AND \
                 signal_value_totals unchanged since last iter"
                    .to_string()
            }
        }
    }
}

/// Default threshold for state stalemate (3 iters of identical
/// `value_totals`). 2 would false-positive on a legitimate single
/// noop iter; 3 means "two consecutive iters of no movement".
pub const STATE_STALEMATE_THRESHOLD: usize = 3;

/// Default threshold for thrashing detection (2 consecutive
/// regression rollbacks). Rollback is rare enough that two-in-a-row
/// is itself the alarm.
pub const THRASHING_THRESHOLD: usize = 2;

/// Outcome of one loop step's decision computation.
///
/// `stalemate_detected` and `thrashing_detected` flow into the
/// IterationEvent emitted for this step. `terminate_reason`
/// signals "stop the loop"; the caller still emits the event before
/// returning so the trace contains the decisive turn.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StepDecision {
    pub stalemate_detected: bool,
    pub thrashing_detected: bool,
    pub terminate_reason: Option<TerminateReason>,
}

/// Rolling history the loop maintains across iterations. Two
/// parallel vectors — one of value_totals snapshots, one of
/// regressed-now booleans — fed to the three sequence detectors.
#[derive(Clone, Debug, Default)]
pub struct LoopState {
    pub value_totals_history: Vec<BTreeMap<String, f64>>,
    pub regressed_history: Vec<bool>,
}

impl LoopState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the just-completed iteration's observations. Call
    /// this AFTER `step_decision` (the detectors look at history
    /// *not* including the current step).
    pub fn record(&mut self, value_totals: BTreeMap<String, f64>, regressed: bool) {
        self.value_totals_history.push(value_totals);
        self.regressed_history.push(regressed);
    }

    pub fn iterations_recorded(&self) -> usize {
        self.value_totals_history.len()
    }
}

/// Pure step-decision computation. See module-level docs for the
/// rationale.
///
/// Order of precedence when both fire:
///   1. **Thrashing** — dominant. Two regression rollbacks in a row
///      are a clearer "stop now" signal than any stalemate.
///   2. **State stalemate** — signal_value_totals unchanged for
///      `STATE_STALEMATE_THRESHOLD` iters. The primary stall signal.
///   3. **Plan-repeat stalemate** — only fires when the plan is
///      byte-identical to the previous one AND state hasn't moved
///      since. A *supporting* signal, not a primary trigger; a
///      single plan repeat without state-stillness is too noisy.
pub fn step_decision(
    state: &LoopState,
    current_value_totals: &BTreeMap<String, f64>,
    regressed_now: bool,
    plan_repeated_now: bool,
) -> StepDecision {
    let state_stalemate =
        is_state_stalemate(&state.value_totals_history, current_value_totals);
    let plan_repeat_stalemate = is_plan_repeat_stalemate(
        plan_repeated_now,
        &state.value_totals_history,
        current_value_totals,
    );
    let thrashing = is_thrashing(&state.regressed_history, regressed_now);
    let stalemate = state_stalemate || plan_repeat_stalemate;

    let terminate_reason = if thrashing {
        Some(TerminateReason::Thrashing)
    } else if stalemate {
        if plan_repeat_stalemate && !state_stalemate {
            Some(TerminateReason::PlanRepeatStalemate)
        } else {
            Some(TerminateReason::StateStalemate)
        }
    } else {
        None
    };

    StepDecision {
        stalemate_detected: stalemate,
        thrashing_detected: thrashing,
        terminate_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn fresh_state_does_not_terminate() {
        let state = LoopState::new();
        let d = step_decision(&state, &vt(&[("x", 1.0)]), false, false);
        assert_eq!(d, StepDecision::default());
    }

    #[test]
    fn three_iters_unchanged_value_totals_terminates_state_stalemate() {
        let mut state = LoopState::new();
        state.record(vt(&[("x", 1.0)]), false);
        state.record(vt(&[("x", 1.0)]), false);
        let d = step_decision(&state, &vt(&[("x", 1.0)]), false, false);
        assert!(d.stalemate_detected);
        assert!(!d.thrashing_detected);
        assert_eq!(d.terminate_reason, Some(TerminateReason::StateStalemate));
    }

    #[test]
    fn two_consecutive_regressions_trigger_thrashing() {
        let mut state = LoopState::new();
        state.record(vt(&[("x", 1.0)]), true);
        let d = step_decision(&state, &vt(&[("x", 2.0)]), true, false);
        assert!(d.thrashing_detected);
        assert_eq!(d.terminate_reason, Some(TerminateReason::Thrashing));
    }

    #[test]
    fn thrashing_dominates_stalemate_when_both_fire() {
        let mut state = LoopState::new();
        state.record(vt(&[("x", 1.0)]), true);
        state.record(vt(&[("x", 1.0)]), false);
        // current iter: same value_totals (would be stalemate) AND
        // regressed_now=true. Need two consecutive regressions for
        // thrashing — the second-to-last record is true, this iter
        // is true, so threshold of 2 met.
        let mut state2 = LoopState::new();
        state2.record(vt(&[("x", 1.0)]), true);
        state2.record(vt(&[("x", 1.0)]), false);
        // Force the history to have a trailing true so thrashing fires.
        state2.regressed_history.pop();
        state2.regressed_history.push(true);
        let d = step_decision(&state2, &vt(&[("x", 1.0)]), true, true);
        assert!(d.thrashing_detected);
        assert!(d.stalemate_detected); // both can be set simultaneously
        // But terminate_reason picks thrashing.
        assert_eq!(d.terminate_reason, Some(TerminateReason::Thrashing));
    }

    #[test]
    fn plan_repeat_with_state_stillness_terminates_plan_repeat_stalemate() {
        // Only one prior iter recorded → not enough for state
        // stalemate's threshold of 3, so plan_repeat is the
        // discriminating signal.
        let mut state = LoopState::new();
        state.record(vt(&[("x", 1.0)]), false);
        let d = step_decision(&state, &vt(&[("x", 1.0)]), false, true);
        assert!(d.stalemate_detected);
        assert_eq!(
            d.terminate_reason,
            Some(TerminateReason::PlanRepeatStalemate)
        );
    }

    #[test]
    fn plan_repeat_with_state_movement_does_not_terminate() {
        let mut state = LoopState::new();
        state.record(vt(&[("x", 1.0)]), false);
        // value_totals moved → plan-repeat alone isn't enough.
        let d = step_decision(&state, &vt(&[("x", 2.0)]), false, true);
        assert!(!d.stalemate_detected);
        assert!(d.terminate_reason.is_none());
    }

    #[test]
    fn terminate_messages_match_python_word_for_word() {
        // Pinned by external tools that pattern-match on result.error;
        // do NOT casually rephrase.
        assert!(TerminateReason::Thrashing
            .as_message()
            .contains("consecutive regression rollbacks"));
        assert!(TerminateReason::StateStalemate
            .as_message()
            .contains("signal_value_totals unchanged"));
        assert!(TerminateReason::PlanRepeatStalemate
            .as_message()
            .contains("planner repeated identical plan"));
    }
}
