"""
Unit tests for decision_pattern.derive_pattern.

The deriver is exhaustive over the boolean shapes IterationEvent can
take. Each branch maps to one named pattern; UNKNOWN exists only as
a safety valve. These tests pin every observable shape so future
changes that add a flag have to consciously update the deriver
(or the test forces them to).
"""
from __future__ import annotations

from aegis.runtime.decision_pattern import DecisionPattern, derive_pattern
from aegis.runtime.pipeline import IterationEvent


def _ev(**overrides) -> IterationEvent:
    base = dict(
        iteration=0,
        plan_id="00000000",
        plan_done=False,
        plan_patches=1,
        validation_passed=True,
        applied=False,
        rolled_back=False,
        regressed=False,
    )
    base.update(overrides)
    return IterationEvent(**base)


def test_applied_done():
    ev = _ev(applied=True, plan_done=True, validation_passed=True)
    assert derive_pattern(ev) == DecisionPattern.APPLIED_DONE


def test_applied_continuing():
    ev = _ev(applied=True, plan_done=False, validation_passed=True)
    assert derive_pattern(ev) == DecisionPattern.APPLIED_CONTINUING


def test_regression_rollback():
    """The defining shape of regression_rollback scenario."""
    ev = _ev(applied=True, rolled_back=True, regressed=True, validation_passed=True)
    assert derive_pattern(ev) == DecisionPattern.REGRESSION_ROLLBACK


def test_executor_failure_after_apply():
    """Executor reported success then something else rolled it back —
    the regression check failed but `regressed` flag was not set."""
    ev = _ev(applied=True, rolled_back=True, regressed=False, validation_passed=True)
    assert derive_pattern(ev) == DecisionPattern.EXECUTOR_FAILURE


def test_executor_failure_during_apply():
    """Executor mid-failure: never claimed applied=True, but rollback
    fired (state restoration of partial writes)."""
    ev = _ev(applied=False, rolled_back=True, regressed=False)
    assert derive_pattern(ev) == DecisionPattern.EXECUTOR_FAILURE


def test_silent_done_veto():
    """The lod_refactor iter-0 shape: planner says done, validator vetoes,
    patches were present (so it is not noop_done)."""
    ev = _ev(
        applied=False,
        plan_done=True,
        plan_patches=1,
        validation_passed=False,
    )
    assert derive_pattern(ev) == DecisionPattern.SILENT_DONE_VETO


def test_validation_veto():
    ev = _ev(applied=False, plan_done=False, validation_passed=False)
    assert derive_pattern(ev) == DecisionPattern.VALIDATION_VETO


def test_silent_done_takes_priority_over_validation_veto():
    """Both branches match; SILENT_DONE_VETO is the more specific label
    and must win. Regression guard for derivation order."""
    ev = _ev(
        applied=False,
        plan_done=True,
        plan_patches=2,
        validation_passed=False,
    )
    assert derive_pattern(ev) == DecisionPattern.SILENT_DONE_VETO


def test_noop_done():
    """Planner declared done with empty patch list — short-circuit."""
    ev = _ev(plan_done=True, plan_patches=0, validation_passed=True)
    assert derive_pattern(ev) == DecisionPattern.NOOP_DONE


def test_noop_done_distinct_from_silent_done_veto():
    """plan_done with empty patches is NOOP_DONE; with patches +
    failed validation it would be SILENT_DONE_VETO. Boundary check."""
    noop = _ev(plan_done=True, plan_patches=0, validation_passed=True)
    silent = _ev(plan_done=True, plan_patches=1, applied=False, validation_passed=False)
    assert derive_pattern(noop) == DecisionPattern.NOOP_DONE
    assert derive_pattern(silent) == DecisionPattern.SILENT_DONE_VETO


def test_stalemate_detected_flag_overrides_mechanical_pattern():
    """Sequence-level meta-decision wins. Even an APPLIED_DONE-shaped
    iteration (applied + plan_done) reports STALEMATE_DETECTED if the
    pipeline observed the loop wasn't moving. The meta-observation is
    the more honest description."""
    ev = _ev(
        applied=True, plan_done=True, validation_passed=True,
        stalemate_detected=True,
    )
    assert derive_pattern(ev) == DecisionPattern.STALEMATE_DETECTED


def test_thrashing_detected_flag_overrides_mechanical_pattern():
    """Same priority rule: a regression_rollback shape with
    thrashing_detected reports THRASHING, not REGRESSION_ROLLBACK.
    The first frames it as a one-off; the second frames it as a
    pattern. Pattern is the truth."""
    ev = _ev(
        applied=True, rolled_back=True, regressed=True,
        thrashing_detected=True,
    )
    assert derive_pattern(ev) == DecisionPattern.THRASHING_DETECTED


def test_thrashing_outranks_stalemate_when_both_set():
    """Order rule: thrashing dominates stalemate. A run that both
    regressed repeatedly *and* failed to make progress is labelled
    by the active failure mode, not the passive one. Mirrors the
    derive_pattern docstring's order-of-checks reasoning."""
    ev = _ev(
        applied=True, rolled_back=True, regressed=True,
        stalemate_detected=True,
        thrashing_detected=True,
    )
    assert derive_pattern(ev) == DecisionPattern.THRASHING_DETECTED


# ---------- pipeline-level detector helpers ----------

def test_state_stalemate_helper_threshold_3():
    from aegis.runtime.pipeline import _is_state_stalemate
    # Empty / under-threshold history → never stalemate.
    assert _is_state_stalemate([], {"a": 1.0}) is False
    assert _is_state_stalemate([{"a": 1.0}], {"a": 1.0}) is False
    # Exactly two prior identical observations + identical current = stalemate.
    assert _is_state_stalemate(
        [{"a": 1.0}, {"a": 1.0}], {"a": 1.0}
    ) is True
    # Any prior observation different breaks stalemate.
    assert _is_state_stalemate(
        [{"a": 2.0}, {"a": 1.0}], {"a": 1.0}
    ) is False
    # Beyond threshold still True if recent N-1 all match current.
    assert _is_state_stalemate(
        [{"a": 0.0}, {"a": 1.0}, {"a": 1.0}], {"a": 1.0}
    ) is True


def test_thrashing_helper_threshold_2():
    from aegis.runtime.pipeline import _is_thrashing
    # Current not regressed → never thrashing.
    assert _is_thrashing([True, True], regressed_now=False) is False
    # Empty history → False (need at least 1 prior True).
    assert _is_thrashing([], regressed_now=True) is False
    # 1 prior True + current True = 2 in a row = thrashing.
    assert _is_thrashing([True], regressed_now=True) is True
    # 1 prior False + current True = isolated regression, not thrashing.
    assert _is_thrashing([False], regressed_now=True) is False


def test_plan_repeat_stalemate_requires_state_no_movement():
    """Plan-repeat is a supporting signal, not a primary trigger.
    Catching the LLM giving us the same bytes twice means nothing
    on its own — we also need the *state* to confirm nothing is
    moving."""
    from aegis.runtime.pipeline import _is_plan_repeat_stalemate
    # No plan repeat → never fires.
    assert _is_plan_repeat_stalemate(
        plan_repeated_now=False,
        value_totals_history=[{"a": 1.0}, {"a": 1.0}],
        current_value_totals={"a": 1.0},
    ) is False
    # Plan repeat + empty history → False (nothing to compare).
    assert _is_plan_repeat_stalemate(
        plan_repeated_now=True,
        value_totals_history=[],
        current_value_totals={"a": 1.0},
    ) is False
    # Plan repeat + state moved last iter → False (plan repeat noise,
    # not real stalemate — the state is responding to something).
    assert _is_plan_repeat_stalemate(
        plan_repeated_now=True,
        value_totals_history=[{"a": 2.0}],
        current_value_totals={"a": 1.0},
    ) is False
    # Plan repeat + state unchanged since last iter → real stalemate.
    assert _is_plan_repeat_stalemate(
        plan_repeated_now=True,
        value_totals_history=[{"a": 1.0}],
        current_value_totals={"a": 1.0},
    ) is True


def test_pattern_values_are_stable_strings():
    """ROADMAP §6.3 spirit: existing reason/label codes must not change.
    Pinning every value here so renames are visible in diff."""
    expected = {
        "applied_done", "applied_continuing", "regression_rollback",
        "executor_failure", "silent_done_veto", "validation_veto",
        "noop_done",
        # Sequence-level meta-decisions added by Gap 1.
        "stalemate_detected", "thrashing_detected",
        "unknown",
    }
    assert {p.value for p in DecisionPattern} == expected
