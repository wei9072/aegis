//! DecisionPattern — the named shapes of one pipeline iteration.
//!
//! See `aegis/runtime/decision_pattern.py` for the full design
//! commentary. This file mirrors the Python derivation order
//! exactly; renames are breaking changes for the trace JSON
//! consumers.

use serde::{Deserialize, Serialize};

use crate::iteration::IterationEvent;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionPattern {
    AppliedDone,
    AppliedContinuing,
    RegressionRollback,
    ExecutorFailure,
    SilentDoneVeto,
    ValidationVeto,
    NoopDone,
    StalemateDetected,
    ThrashingDetected,
    Unknown,
}

impl DecisionPattern {
    /// Stable string label. Renames break trace consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionPattern::AppliedDone => "applied_done",
            DecisionPattern::AppliedContinuing => "applied_continuing",
            DecisionPattern::RegressionRollback => "regression_rollback",
            DecisionPattern::ExecutorFailure => "executor_failure",
            DecisionPattern::SilentDoneVeto => "silent_done_veto",
            DecisionPattern::ValidationVeto => "validation_veto",
            DecisionPattern::NoopDone => "noop_done",
            DecisionPattern::StalemateDetected => "stalemate_detected",
            DecisionPattern::ThrashingDetected => "thrashing_detected",
            DecisionPattern::Unknown => "unknown",
        }
    }

    /// Inverse of `as_str`. Returns `None` for unknown labels.
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "applied_done" => DecisionPattern::AppliedDone,
            "applied_continuing" => DecisionPattern::AppliedContinuing,
            "regression_rollback" => DecisionPattern::RegressionRollback,
            "executor_failure" => DecisionPattern::ExecutorFailure,
            "silent_done_veto" => DecisionPattern::SilentDoneVeto,
            "validation_veto" => DecisionPattern::ValidationVeto,
            "noop_done" => DecisionPattern::NoopDone,
            "stalemate_detected" => DecisionPattern::StalemateDetected,
            "thrashing_detected" => DecisionPattern::ThrashingDetected,
            "unknown" => DecisionPattern::Unknown,
            _ => return None,
        })
    }
}

/// Map one IterationEvent to exactly one DecisionPattern. Order of
/// checks matches `aegis/runtime/decision_pattern.py::derive_pattern`.
pub fn derive_pattern(ev: &IterationEvent) -> DecisionPattern {
    if ev.thrashing_detected {
        return DecisionPattern::ThrashingDetected;
    }
    if ev.stalemate_detected {
        return DecisionPattern::StalemateDetected;
    }
    if ev.applied && !ev.rolled_back {
        return if ev.plan_done {
            DecisionPattern::AppliedDone
        } else {
            DecisionPattern::AppliedContinuing
        };
    }
    if ev.applied && ev.rolled_back {
        return if ev.regressed {
            DecisionPattern::RegressionRollback
        } else {
            DecisionPattern::ExecutorFailure
        };
    }
    if ev.rolled_back {
        return DecisionPattern::ExecutorFailure;
    }
    if ev.silent_done_contradiction {
        return DecisionPattern::SilentDoneVeto;
    }
    if ev.plan_done && ev.plan_patches == 0 && ev.validation_passed {
        return DecisionPattern::NoopDone;
    }
    if !ev.validation_passed {
        return DecisionPattern::ValidationVeto;
    }
    DecisionPattern::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> IterationEvent {
        IterationEvent {
            validation_passed: true,
            plan_patches: 1,
            ..Default::default()
        }
    }

    #[test]
    fn applied_done() {
        let mut e = ev();
        e.applied = true;
        e.plan_done = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::AppliedDone);
    }

    #[test]
    fn applied_continuing() {
        let mut e = ev();
        e.applied = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::AppliedContinuing);
    }

    #[test]
    fn regression_rollback() {
        let mut e = ev();
        e.applied = true;
        e.rolled_back = true;
        e.regressed = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::RegressionRollback);
    }

    #[test]
    fn executor_failure_after_apply() {
        let mut e = ev();
        e.applied = true;
        e.rolled_back = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::ExecutorFailure);
    }

    #[test]
    fn executor_failure_during_apply() {
        let mut e = ev();
        e.rolled_back = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::ExecutorFailure);
    }

    #[test]
    fn silent_done_veto() {
        let mut e = ev();
        e.plan_done = true;
        e.plan_patches = 1;
        e.validation_passed = false;
        e.silent_done_contradiction = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::SilentDoneVeto);
    }

    #[test]
    fn validation_veto() {
        let mut e = ev();
        e.validation_passed = false;
        assert_eq!(derive_pattern(&e), DecisionPattern::ValidationVeto);
    }

    #[test]
    fn noop_done() {
        let mut e = ev();
        e.plan_done = true;
        e.plan_patches = 0;
        assert_eq!(derive_pattern(&e), DecisionPattern::NoopDone);
    }

    #[test]
    fn stalemate_overrides_mechanical() {
        let mut e = ev();
        e.applied = true;
        e.plan_done = true;
        e.stalemate_detected = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::StalemateDetected);
    }

    #[test]
    fn thrashing_overrides_stalemate() {
        let mut e = ev();
        e.applied = true;
        e.rolled_back = true;
        e.regressed = true;
        e.stalemate_detected = true;
        e.thrashing_detected = true;
        assert_eq!(derive_pattern(&e), DecisionPattern::ThrashingDetected);
    }

    #[test]
    fn pattern_string_roundtrip() {
        let labels = [
            "applied_done",
            "applied_continuing",
            "regression_rollback",
            "executor_failure",
            "silent_done_veto",
            "validation_veto",
            "noop_done",
            "stalemate_detected",
            "thrashing_detected",
            "unknown",
        ];
        for label in labels {
            let p = DecisionPattern::from_str(label).expect("known label");
            assert_eq!(p.as_str(), label);
        }
    }
}
