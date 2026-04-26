//! Sequence-level detectors — Gap 1 (STALEMATE / THRASHING) logic.
//!
//! Pure functions over the recent iteration history. Mirrors the
//! Python helpers in `aegis/runtime/pipeline.py` exactly so callers
//! that gain a Rust loop later (V1.3+) get identical decisions.

use std::collections::BTreeMap;

const STATE_STALEMATE_THRESHOLD: usize = 3;
const THRASHING_THRESHOLD: usize = 2;

/// `value_totals` per iteration are dicts of `kind → total` (where
/// kind is e.g. `"fan_out"`, `"max_chain_depth"`). Stalemate fires
/// when the last `(threshold - 1)` historical observations all
/// match the current observation — i.e. the system has produced
/// `threshold` consecutive identical state snapshots.
pub fn is_state_stalemate(
    history: &[BTreeMap<String, f64>],
    current_value_totals: &BTreeMap<String, f64>,
) -> bool {
    let needed = STATE_STALEMATE_THRESHOLD - 1;
    if history.len() < needed {
        return false;
    }
    let recent = &history[history.len() - needed..];
    recent.iter().all(|h| h == current_value_totals)
}

/// Thrashing fires when the most recent `(threshold - 1)` events
/// AND the current event were all regressions — `threshold`
/// consecutive regressions in a row.
pub fn is_thrashing(history: &[bool], regressed_now: bool) -> bool {
    if !regressed_now {
        return false;
    }
    let needed = THRASHING_THRESHOLD - 1;
    if history.len() < needed {
        return false;
    }
    let recent = &history[history.len() - needed..];
    recent.iter().all(|r| *r)
}

/// Plan-repeat is a *supporting* signal — by itself it can mean
/// the LLM is being deterministic on identical input, not stuck.
/// Stalemate by plan-repeat only fires when the state is also
/// confirmed not to be moving.
pub fn is_plan_repeat_stalemate(
    plan_repeated_now: bool,
    value_totals_history: &[BTreeMap<String, f64>],
    current_value_totals: &BTreeMap<String, f64>,
) -> bool {
    if !plan_repeated_now {
        return false;
    }
    let last = match value_totals_history.last() {
        Some(v) => v,
        None => return false,
    };
    last == current_value_totals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt(items: &[(&str, f64)]) -> BTreeMap<String, f64> {
        items.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn state_stalemate_threshold_3() {
        // Empty / under-threshold → false
        assert!(!is_state_stalemate(&[], &vt(&[("a", 1.0)])));
        assert!(!is_state_stalemate(&[vt(&[("a", 1.0)])], &vt(&[("a", 1.0)])));
        // Exactly two prior identical + current identical = stalemate
        assert!(is_state_stalemate(
            &[vt(&[("a", 1.0)]), vt(&[("a", 1.0)])],
            &vt(&[("a", 1.0)])
        ));
        // Any prior different → false
        assert!(!is_state_stalemate(
            &[vt(&[("a", 2.0)]), vt(&[("a", 1.0)])],
            &vt(&[("a", 1.0)])
        ));
        // Beyond threshold still True if recent N-1 match current
        assert!(is_state_stalemate(
            &[vt(&[("a", 0.0)]), vt(&[("a", 1.0)]), vt(&[("a", 1.0)])],
            &vt(&[("a", 1.0)])
        ));
    }

    #[test]
    fn thrashing_threshold_2() {
        assert!(!is_thrashing(&[true, true], false));
        assert!(!is_thrashing(&[], true));
        assert!(is_thrashing(&[true], true));
        assert!(!is_thrashing(&[false], true));
    }

    #[test]
    fn plan_repeat_stalemate_requires_state_no_movement() {
        assert!(!is_plan_repeat_stalemate(
            false,
            &[vt(&[("a", 1.0)]), vt(&[("a", 1.0)])],
            &vt(&[("a", 1.0)])
        ));
        assert!(!is_plan_repeat_stalemate(true, &[], &vt(&[("a", 1.0)])));
        assert!(!is_plan_repeat_stalemate(
            true,
            &[vt(&[("a", 2.0)])],
            &vt(&[("a", 1.0)])
        ));
        assert!(is_plan_repeat_stalemate(
            true,
            &[vt(&[("a", 1.0)])],
            &vt(&[("a", 1.0)])
        ));
    }
}
