"""
Example 04 — Reading the decision trace.

Every iteration of the pipeline emits an IterationEvent that carries
a named DecisionPattern (one of 9 named shapes: APPLIED_DONE,
REGRESSION_ROLLBACK, STALEMATE_DETECTED, ...). Consuming these is how
you build dashboards, logs, audit trails, or escalation policies on
top of Aegis without modifying its internals.

This example uses the on_iteration callback to render a one-line
summary per iter — the same shape `aegis scenario run` displays.

Run from the repo root:
    python examples/04_read_decision_trace.py
"""
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from aegis.agents.gemini import GeminiProvider  # noqa: E402
from aegis.runtime import pipeline  # noqa: E402
from aegis.runtime.decision_pattern import DecisionPattern  # noqa: E402


_LABEL = {
    DecisionPattern.APPLIED_DONE:        "✓ applied + planner done",
    DecisionPattern.APPLIED_CONTINUING:  "→ applied, continuing",
    DecisionPattern.REGRESSION_ROLLBACK: "↻ rolled back (signals worsened)",
    DecisionPattern.EXECUTOR_FAILURE:    "↻ rolled back (executor error)",
    DecisionPattern.SILENT_DONE_VETO:    "✗ planner lied about done",
    DecisionPattern.VALIDATION_VETO:     "✗ validator rejected plan",
    DecisionPattern.NOOP_DONE:           "○ planner: nothing to do",
    DecisionPattern.STALEMATE_DETECTED:  "⌛ stalemate — terminating",
    DecisionPattern.THRASHING_DETECTED:  "⚠ thrashing — terminating",
    DecisionPattern.UNKNOWN:             "? unknown (deriver gap)",
}


def render(ev) -> None:
    print(f"  iter {ev.iteration}  {_LABEL.get(ev.decision_pattern, ev.decision_pattern.value):40}"
          f"  plan={ev.plan_id}  patches={ev.plan_patches}")


def main() -> None:
    workspace = Path(__file__).parent / "_scratch_trace"
    workspace.mkdir(exist_ok=True)
    (workspace / "broken.py").write_text("def add(a, b)\n    return a + b\n", encoding="utf-8")

    result = pipeline.run(
        task="Fix the syntax error in broken.py minimally.",
        root=str(workspace),
        provider=GeminiProvider(model_name="gemma-4-31b-it"),
        on_iteration=render,
    )

    print(f"\nfinal: success={result.success} after {result.iterations} iter")


if __name__ == "__main__":
    main()
