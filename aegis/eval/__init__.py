"""
Aegis evaluation harness.

The eval harness verifies *decision quality*, not output text:
each scenario asserts that the system's DecisionTrace contains a
specific ordered subsequence of events. New observability events do
not break old scenarios (subsequence match), but missing or wrong
gate decisions do.

Public surface:
    Scenario, ExpectedEvent, ScenarioResult,
    run_all, format_results
"""
from aegis.eval.harness import (
    ExpectedEvent,
    Scenario,
    ScenarioResult,
    format_results,
    run_all,
)

__all__ = [
    "ExpectedEvent",
    "Scenario",
    "ScenarioResult",
    "format_results",
    "run_all",
]
