"""
Scenario: syntax_fix.

Why this is the first scenario, not fanout_reduce: the convergence is
binary (parses or doesn't), so any non-convergence isolates a
pipeline-infrastructure problem cleanly. fanout_reduce is heuristic
and noisy — running it before the infrastructure is trusted would
muddle "system bug" with "model wobble".

Seed state: `input/broken.py` is missing the colon on `def add(a, b)`.
The other function (`multiply`) is fine, so a correct refactor only
touches one line.

Success criteria (interpretation, not asserted by runner):
  - planner emits a patch that adds the missing colon (or rewrites
    the function header)
  - validator passes that patch
  - executor applies it
  - re-extracted signals do not regress (count must not increase)
  - planner declares done by iteration 2 at latest
"""
from __future__ import annotations

from pathlib import Path

from tests.scenarios._runner import MultiTurnScenario


HERE = Path(__file__).parent

SCENARIO = MultiTurnScenario(
    name="syntax_fix",
    description=(
        "broken.py is missing a colon on the `def add` header. "
        "Refactor pipeline should produce one patch that adds it, "
        "and converge in one or two iterations."
    ),
    input_dir=HERE / "input",
    task=(
        "There is a Python syntax error in broken.py. The function "
        "`def add(a, b)` is missing a colon at the end of its header. "
        "Produce a minimal patch that fixes the syntax error so the "
        "file parses cleanly. Do not modify any other function."
    ),
    max_iterations=3,
    expectations={
        "must_converge_within": 2,
        "final_pipeline_success": True,
        # signal count is non-decreasing on a fix-only refactor; equal is fine.
        "no_signal_regression": True,
    },
)
