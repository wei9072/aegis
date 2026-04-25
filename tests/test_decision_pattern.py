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


def test_pattern_values_are_stable_strings():
    """ROADMAP §6.3 spirit: existing reason/label codes must not change.
    Pinning every value here so renames are visible in diff."""
    expected = {
        "applied_done", "applied_continuing", "regression_rollback",
        "executor_failure", "silent_done_veto", "validation_veto",
        "noop_done", "unknown",
    }
    assert {p.value for p in DecisionPattern} == expected
