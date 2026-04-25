"""
Scenario: fanout_reduce.

Now that syntax_fix establishes a 1-iteration system-correctness
baseline, this scenario probes a different axis: heuristic
optimization. fan_out is a Ring 0.5 advisory signal; the task here
asks the LLM to reduce it by removing imports that `do_thing`
doesn't reference.

Where syntax_fix is binary (parses or doesn't), fan_out is a
gradient — partial fixes are possible, the LLM might split the work
across iterations, and convergence is not guaranteed. That's the
point: this scenario observes whether the iteration loop produces
monotonic improvement, oscillation, or stalemate, and whether
PolicyEngine's `fan_out >= 10 → warn` translates into something the
Planner can productively act on.

Seed state: `input/service.py` imports 15 stdlib modules at top
level but `do_thing` only uses two of them (`os`, `time`).

Expected (interpretation, not asserted by runner):
  - iter 0: planner removes the unused imports, fan_out drops to 2
  - planner declares done by iter 2 at latest

If the run shows oscillation (signal_delta_vs_prev flipping sign)
or repeated stalemates, that is a *capability* finding worth
recording — but not an infrastructure bug, since syntax_fix proves
the loop itself is sound.
"""
from __future__ import annotations

from pathlib import Path

from aegis.runtime.decision_pattern import DecisionPattern
from tests.scenarios._runner import MultiTurnScenario


HERE = Path(__file__).parent

SCENARIO = MultiTurnScenario(
    name="fanout_reduce",
    description=(
        "service.py imports 15 stdlib modules but only uses two of "
        "them. Refactor pipeline should drop the unused imports, "
        "lowering fan_out from 15 to 2."
    ),
    input_dir=HERE / "input",
    task=(
        "service.py imports 15 different stdlib modules at the top "
        "but the `do_thing` function only references `os` and "
        "`time`. Reduce the file's fan-out by deleting every import "
        "that `do_thing` does not actually use. Keep `do_thing` "
        "behaviour identical."
    ),
    max_iterations=3,
    expectations={
        "must_converge_within": 3,
        "final_pipeline_success": True,
        "fan_out_must_decrease": True,
        "fan_out_target_at_most": 5,  # generous; ideal would be 2
    },
    expected_patterns=[DecisionPattern.APPLIED_DONE],
)
