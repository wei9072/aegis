"""
Tests for the eval harness itself + the 10 built-in scenarios.

The 'all built-in scenarios pass' test pins current trace shape: any
future change that breaks gate behaviour will fail here, which is
exactly the regression signal we want before adding new layers.
"""
import pytest

from aegis.eval import (
    ExpectedEvent,
    Scenario,
    ScenarioResult,
    format_results,
    run_all,
)
from aegis.eval.scenarios import SCENARIOS


# ---------- ExpectedEvent matching ----------

def _evt(layer, decision, reason="", metadata=None):
    """Build a DecisionEvent without instantiating via the trace API."""
    from aegis.runtime.trace import DecisionEvent
    return DecisionEvent(
        layer=layer,
        decision=decision,
        reason=reason,
        metadata=metadata or {},
    )


def test_expected_matches_layer_and_decision_only():
    e = ExpectedEvent("ring0", "pass")
    assert e.matches(_evt("ring0", "pass", reason="anything"))
    assert not e.matches(_evt("ring0", "block"))
    assert not e.matches(_evt("ring0_5", "pass"))


def test_expected_matches_with_reason_constraint():
    e = ExpectedEvent("ring0", "pass", reason="syntax_valid")
    assert e.matches(_evt("ring0", "pass", reason="syntax_valid"))
    assert not e.matches(_evt("ring0", "pass", reason="non_code_response"))


def test_expected_matches_metadata_includes():
    e = ExpectedEvent(
        "provider", "observe", "tool_surface",
        metadata_includes={"tools": ["read_file"]},
    )
    assert e.matches(_evt(
        "provider", "observe", "tool_surface",
        metadata={"tools": ["read_file"], "attempt": 1},
    ))
    assert not e.matches(_evt(
        "provider", "observe", "tool_surface",
        metadata={"tools": []},
    ))


# ---------- Subsequence matching tolerates extra events ----------

def test_scenario_passes_when_actual_contains_extra_observability():
    """Adding a new observability event must NOT break old scenarios."""
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["x = 1"],
        expected_events=[
            ExpectedEvent("ring0", "pass"),
            ExpectedEvent("gateway", "pass", "response_accepted"),
        ],
    )
    result = s.run()
    assert result.passed, result.mismatches


def test_scenario_fails_when_required_event_missing():
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["x = 1"],
        expected_events=[
            # this never happens for valid code
            ExpectedEvent("ring0", "block"),
        ],
    )
    result = s.run()
    assert not result.passed
    assert any("missing expected" in m for m in result.mismatches)


def test_scenario_fails_when_order_wrong():
    """Subsequence must respect order — ring0 pass cannot precede request_started."""
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["x = 1"],
        expected_events=[
            ExpectedEvent("gateway", "pass", "response_accepted"),
            ExpectedEvent("gateway", "observe", "request_started"),  # too late
        ],
    )
    result = s.run()
    assert not result.passed


# ---------- Raise expectations ----------

def test_scenario_expects_raise_when_max_retries_exhausted():
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["```python\ndef bad(\n```"] * 3,
        max_retries=3,
        expects_raise=True,
        expected_events=[
            ExpectedEvent("gateway", "block", "max_retries_exhausted"),
        ],
    )
    result = s.run()
    assert result.passed
    assert "RuntimeError" in (result.raised or "")


def test_scenario_fails_if_unexpected_raise():
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["```python\ndef bad(\n```"] * 3,
        expects_raise=False,
        expected_events=[],
    )
    result = s.run()
    assert not result.passed
    assert any("did NOT expect a raise" in m for m in result.mismatches)


def test_scenario_fails_if_expected_raise_did_not_happen():
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["x = 1"],
        expects_raise=True,
        expected_events=[],
    )
    result = s.run()
    assert not result.passed
    assert any("expected gateway to raise" in m for m in result.mismatches)


# ---------- Provider stub records last_used_tools ----------

def test_scenario_records_tools_in_provider_stub():
    from aegis.tools.file_system import read_file
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=["x = 1"],
        tools=(read_file,),
        expected_events=[
            ExpectedEvent(
                "provider", "observe", "tool_surface",
                metadata_includes={"tools": ["read_file"]},
            ),
        ],
    )
    result = s.run()
    assert result.passed, result.mismatches


def test_scenario_runs_out_of_responses_raises():
    s = Scenario(
        name="t",
        description="",
        prompt="x",
        llm_responses=[],  # nothing to consume
        expects_raise=True,
        expected_events=[],
    )
    result = s.run()
    assert "ran out of canned LLM responses" in (result.raised or "")


# ---------- format_results output ----------

def test_format_results_summarises_pass_and_fail():
    results = [
        ScenarioResult(name="ok", passed=True, actual_events=[]),
        ScenarioResult(name="bad", passed=False, actual_events=[],
                       mismatches=["missing X"]),
    ]
    out = format_results(results)
    assert "[PASS] ok" in out
    assert "[FAIL] bad" in out
    assert "missing X" in out
    assert "1/2 scenarios passed" in out


# ---------- All built-in scenarios pass on current code ----------

def test_all_builtin_scenarios_pass_on_current_code():
    """Pin every shipped scenario to current trace shape. Any future
    change that alters gate emission without updating scenarios must
    fail this test loudly."""
    results = run_all(SCENARIOS)
    failed = [r for r in results if not r.passed]
    if failed:
        pytest.fail("scenario regressions:\n" + format_results(results))


def test_builtin_scenarios_count_matches_documented():
    """Pinned to current count so adding/removing a scenario is a
    deliberate edit. Original 10 + 2 from Phase 2.5 (intent labels) +
    1 from Phase 3 (intent-bypass negative coverage) = 13."""
    assert len(SCENARIOS) == 13


def test_builtin_scenario_names_unique():
    names = [s.name for s in SCENARIOS]
    assert len(names) == len(set(names)), "scenario names must be unique"


def test_gap_scenarios_carry_explanatory_notes():
    """Any scenario marked GAP must explain why so the note can be
    converted into a real assertion when the relevant layer ships. A
    GAP-free harness is the design goal — zero is acceptable; absence
    of explanation is not."""
    gap_scenarios = [s for s in SCENARIOS if "GAP" in s.note]
    for s in gap_scenarios:
        assert len(s.note) > 40, (
            f"scenario {s.name} marks a GAP but the note is too short "
            f"to be useful: {s.note!r}"
        )
