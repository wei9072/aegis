"""
Scenario: lod_refactor.

Designed to be the FIRST scenario where the multi-iteration narrative
("iter 1 worse → iter 2 better → 3 converge") can actually appear.
syntax_fix and fanout_reduce both converge in one shot, which proves
the loop runs but does not prove that the *decision system* in the
loop matters — a pass-through pipeline would behave identically.

LoD-style refactors resist single-shot solutions because:

  - There are usually multiple violations of the same shape; the LLM
    might fix one and forget the rest.
  - The "right" fix has design choice — add helpers on User, on
    Profile, on Address, or some mix. Different choices yield
    different fan_out / chain_depth tradeoffs.
  - Partial fixes can leave intermediate state where one metric
    improves and another degrades, exercising the regression check.

Seed state: orders.py holds a four-class graph (Address ← Profile ←
User, plus an OrderProcessor) and three OrderProcessor methods that
all reach through `user.profile.address.X`. chain_depth=4 across
several call sites.

Expected behaviour (interpretation, not asserted):

  - iter 0: LLM produces a partial fix or an over-engineered fix
    (e.g. helper that still chains internally, or only one of three
    methods refactored). chain_depth drops but is not yet at target.
  - iter 1: planner sees the residual signal, completes the fix.
  - iter 2: done.

If it converges in one iteration anyway, that's also a valid
finding — would mean gemma-4-31b-it solves this class of refactor
in a single shot, and we then need a harder scenario to expose
multi-turn decision-making. If it oscillates or stalemates, that's
the most informative outcome of all: the system's reasoning loop
becomes observable.

The task explicitly forbids new files so the pipeline's
instance-count regression rollback is not triggered by an extra
file appearing — multi-iter behaviour, if any, has to come from
"not yet done", not from "rolled back".
"""
from __future__ import annotations

from pathlib import Path

from aegis.runtime.decision_pattern import DecisionPattern
from tests.scenarios._runner import MultiTurnScenario
from tests.scenarios.lod_refactor.verifier import LodRefactorVerifier


HERE = Path(__file__).parent

SCENARIO = MultiTurnScenario(
    name="lod_refactor",
    description=(
        "orders.py has three Law-of-Demeter violations: OrderProcessor "
        "reaches through `user.profile.address.X` in three different "
        "methods. Refactor so OrderProcessor only talks to User."
    ),
    input_dir=HERE / "input",
    task=(
        "orders.py has multiple Law-of-Demeter violations: "
        "OrderProcessor reaches through user.profile.address in "
        "three different methods (ship_to_country, ship_to_city, "
        "country_then_city). Refactor the file so OrderProcessor "
        "only talks to User directly. Add helper methods on User "
        "(and/or Profile and Address) to expose the data instead of "
        "letting OrderProcessor walk the object graph. Constraints: "
        "1) do not create new files — all changes must stay inside "
        "orders.py; 2) preserve the return values of the three "
        "OrderProcessor methods exactly."
    ),
    max_iterations=3,
    expectations={
        "must_converge_within": 3,
        "final_pipeline_success": True,
        "max_chain_depth_must_decrease": True,
        "max_chain_depth_target_at_most": 2,
    },
    # The first scenario where the loop must replan after a veto and
    # then succeed — both patterns must appear, in this order.
    expected_patterns=[
        DecisionPattern.SILENT_DONE_VETO,
        DecisionPattern.APPLIED_DONE,
    ],
    verifier=LodRefactorVerifier(),
)
