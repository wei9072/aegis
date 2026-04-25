"""
Scenario: regression_rollback.

Designed to exercise the path no other scenario has touched yet —
`Executor.apply` succeeds, but `_regressed()` then triggers
`rollback_result()`. The narrative this should produce:

    iter 0: planner splits god_module into per-concern files →
            instance count rises → rollback → iter 0's diff
            is undone, file state reverts to seed
    iter 1: planner sees `previous_regressed=True` (Planner prompt
            handles this branch explicitly), tries a different
            strategy that does not increase signal instance count
            (e.g. in-file class extraction, or method split)
    iter 2: applied; done

This is the first scenario where the system gets to *evaluate its
own work* and undo it. syntax_fix / fanout_reduce / lod_refactor
all moved forward; this one tests whether the loop can move
backward when forward made things worse.

The trap is ecological: "Refactor for clearer separation of
responsibilities" + "each concern should live in an appropriate
location" reads to most LLMs as "make new files". Once the LLM
walks into that, the regression check has to be the thing that
saves it — there's no other gate that fires here (syntax is fine,
no Demeter violation, no fan_out spike).

Three checkpoints to look for in the trace:
  1. iter 0 Apply line says "applied → rolled back (signals
     regressed)" — not "validation failed". This means the patch
     reached disk and was undone, not blocked at the gate.
  2. iter 0 `regressed=True` in the snapshot — the rollback was
     triggered by signal instance-count growth, not by an executor
     error.
  3. iter 1 plan_id ≠ iter 0 plan_id AND the strategy actually
     changes (LLM responds to `previous_regressed` flag, doesn't
     just retry).

If iter 0 happens to in-file split immediately, that's a valid
finding too — would mean the model skips the trap, and we'd need a
sharper task wording to reproduce. Either outcome is informative.
"""
from __future__ import annotations

from pathlib import Path

from tests.scenarios._runner import MultiTurnScenario


HERE = Path(__file__).parent

SCENARIO = MultiTurnScenario(
    name="regression_rollback",
    description=(
        "god_module.py mixes three concerns. Refactoring it tempts "
        "the LLM to split into multiple files, which raises signal "
        "instance count and triggers post-apply rollback."
    ),
    input_dir=HERE / "input",
    task=(
        "god_module.py mixes three unrelated concerns: User identity, "
        "Billing, and Notification. Refactor the codebase for clearer "
        "separation of responsibilities — each concern should live in "
        "an appropriate location. Preserve all existing function "
        "behaviour."
    ),
    max_iterations=3,
    expectations={
        "must_converge_within": 3,
        "rollback_path_exercised": True,  # iter 0 should rollback
        "post_rollback_strategy_change": True,  # iter 1 should change tactic
    },
)
