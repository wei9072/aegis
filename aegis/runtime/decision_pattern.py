"""
DecisionPattern — the named shapes of one pipeline iteration.

The four-scenario evidence set (syntax_fix / fanout_reduce /
lod_refactor / regression_rollback) revealed that every iteration
falls into one of seven discrete decision shapes. Naming them turns
"the pipeline made a decision" from a narrative observation into a
first-class, machine-checkable data point.

Two consequences worth holding in mind:

  - Scenario expectations can now assert decision *paths*, not just
    output state. `regression_rollback` expects at least one
    REGRESSION_ROLLBACK event; if a future change accidentally turns
    that scenario into a 1-iter success, the assertion fails even
    though the file might look correct.
  - Trace summaries compress: "VALIDATION_VETO → APPLIED_DONE" tells
    you the same story as 20 lines of narrative.

The enum is intentionally exhaustive over current pipeline behaviour
— every IterationEvent is mapped to exactly one pattern. UNKNOWN is
a safety valve for future code paths the deriver hasn't been taught
about yet; if it ever fires, the deriver needs an update, not a
silent fallback.
"""
from __future__ import annotations

from enum import Enum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from aegis.runtime.pipeline import IterationEvent


class DecisionPattern(str, Enum):
    """Named shapes of one pipeline iteration.

    Names are stable; downstream tooling (trace JSON snapshots,
    scenario assertions, summary lines) refers to these strings.
    Renaming an existing pattern is a breaking change.
    """

    # Patch reached disk and stayed there.
    APPLIED_DONE = "applied_done"
    APPLIED_CONTINUING = "applied_continuing"

    # Patch reached disk, then was undone.
    REGRESSION_ROLLBACK = "regression_rollback"  # post-apply signals worsened
    EXECUTOR_FAILURE = "executor_failure"        # apply failed mid-execution

    # Patch never reached disk.
    SILENT_DONE_VETO = "silent_done_veto"  # planner said done, validator disagreed
    VALIDATION_VETO = "validation_veto"    # ordinary validator rejection

    # Planner declared done without any patches.
    NOOP_DONE = "noop_done"

    # Logic gap — should never fire if deriver is exhaustive.
    UNKNOWN = "unknown"


def derive_pattern(ev: "IterationEvent") -> DecisionPattern:
    """Map one IterationEvent to exactly one DecisionPattern.

    Order of checks matters: silent_done_contradiction is more
    specific than the generic VALIDATION_VETO and must be tested
    first, otherwise both branches would match and the more
    informative label would be lost.
    """
    # Patch was applied. Did it stick?
    if ev.applied and not ev.rolled_back:
        return (
            DecisionPattern.APPLIED_DONE
            if ev.plan_done
            else DecisionPattern.APPLIED_CONTINUING
        )

    # Patch was applied but undone. Why?
    if ev.applied and ev.rolled_back:
        return (
            DecisionPattern.REGRESSION_ROLLBACK
            if ev.regressed
            else DecisionPattern.EXECUTOR_FAILURE
        )

    # Patch never applied. Executor mid-failure (state restored,
    # never reported applied=True) is rare but possible.
    if ev.rolled_back:
        return DecisionPattern.EXECUTOR_FAILURE

    # The contradiction case must precede plain VALIDATION_VETO,
    # since silent_done is a strict subset of "validation didn't pass".
    if ev.silent_done_contradiction:
        return DecisionPattern.SILENT_DONE_VETO

    # Planner declared done with empty patch list — pipeline
    # short-circuits at iter start without invoking validator.
    if ev.plan_done and ev.plan_patches == 0 and ev.validation_passed:
        return DecisionPattern.NOOP_DONE

    # Generic plan-rejected path.
    if not ev.validation_passed:
        return DecisionPattern.VALIDATION_VETO

    # Catch-all. If this fires, derivation is missing a branch and
    # someone needs to extend this function — not silently absorb.
    return DecisionPattern.UNKNOWN
